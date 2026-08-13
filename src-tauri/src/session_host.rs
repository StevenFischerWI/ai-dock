use std::{
    collections::{HashMap, VecDeque},
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, BufWriter, Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender, TrySendError},
    },
    thread,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use parking_lot::Mutex;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use uuid::Uuid;

use crate::session_protocol::{
    HostEvent, HostOutputChunk, HostRequest, HostResponse, HostSessionDefinition,
    MAX_BACKLOG_BYTES, Rendezvous, SESSION_HOST_PROTOCOL,
};

const SUBSCRIBER_QUEUE_CAPACITY: usize = 512;

struct SessionRuntime {
    state: String,
    exit_code: Option<u32>,
    message: Option<String>,
    backlog: VecDeque<HostOutputChunk>,
    backlog_bytes: usize,
    next_output_sequence: u64,
    subscribers: HashMap<u64, SyncSender<HostEvent>>,
}

struct HostedSession {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    stop_requested: AtomicBool,
    runtime: Mutex<SessionRuntime>,
}

impl HostedSession {
    fn publish_output(&self, session_id: Uuid, data: String) {
        let mut runtime = self.runtime.lock();
        let sequence = runtime.next_output_sequence;
        runtime.next_output_sequence = runtime.next_output_sequence.saturating_add(1);
        runtime.backlog_bytes += data.len();
        runtime.backlog.push_back(HostOutputChunk {
            sequence,
            data: data.clone(),
        });
        while runtime.backlog_bytes > MAX_BACKLOG_BYTES {
            let Some(removed) = runtime.backlog.pop_front() else {
                break;
            };
            runtime.backlog_bytes = runtime.backlog_bytes.saturating_sub(removed.data.len());
        }
        let event = HostEvent::Output {
            session_id,
            sequence,
            data,
        };
        runtime
            .subscribers
            .retain(|_, subscriber| match subscriber.try_send(event.clone()) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
            });
    }

    fn publish_event(&self, event: HostEvent) {
        let mut runtime = self.runtime.lock();
        runtime
            .subscribers
            .retain(|_, subscriber| match subscriber.try_send(event.clone()) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
            });
    }

    fn set_state(
        &self,
        session_id: Uuid,
        state: &str,
        exit_code: Option<u32>,
        message: Option<String>,
    ) {
        {
            let mut runtime = self.runtime.lock();
            runtime.state = state.to_string();
            runtime.exit_code = exit_code;
            runtime.message.clone_from(&message);
        }
        self.publish_event(HostEvent::State {
            session_id,
            state: state.to_string(),
            exit_code,
            message,
        });
    }
}

struct HostState {
    token: String,
    sessions: Mutex<HashMap<Uuid, Arc<HostedSession>>>,
    next_attachment_id: AtomicU64,
    shutdown: AtomicBool,
    log_path: PathBuf,
}

pub fn run(config_dir: &Path) -> Result<()> {
    fs::create_dir_all(config_dir).context("creating session host configuration directory")?;
    let rendezvous_path = config_dir.join("session-host.json");
    let log_path = config_dir.join("session-host.log");
    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .context("binding the local session host")?;
    listener
        .set_nonblocking(true)
        .context("making the session host listener nonblocking")?;
    let port = listener
        .local_addr()
        .context("reading host address")?
        .port();
    let state = Arc::new(HostState {
        token: format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
        sessions: Mutex::new(HashMap::new()),
        next_attachment_id: AtomicU64::new(1),
        shutdown: AtomicBool::new(false),
        log_path,
    });
    let rendezvous = Rendezvous {
        protocol: SESSION_HOST_PROTOCOL,
        pid: std::process::id(),
        port,
        token: state.token.clone(),
    };
    write_json_atomically(&rendezvous_path, &rendezvous)?;
    log_host(
        &state.log_path,
        &format!("started pid={} port={port}", rendezvous.pid),
    );

    while !state.shutdown.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                let connection_state = state.clone();
                let _ = thread::Builder::new()
                    .name("ai-dock-host-connection".to_string())
                    .spawn(move || {
                        if let Err(error) = handle_connection(stream, &connection_state) {
                            log_host(
                                &connection_state.log_path,
                                &format!("connection error: {error:#}"),
                            );
                        }
                    });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => {
                log_host(&state.log_path, &format!("listener error: {error}"));
                thread::sleep(Duration::from_millis(100));
            }
        }
    }

    stop_all_sessions(&state);
    let _ = fs::remove_file(&rendezvous_path);
    log_host(&state.log_path, "stopped");
    Ok(())
}

fn handle_connection(mut stream: TcpStream, state: &Arc<HostState>) -> Result<()> {
    stream
        .set_nonblocking(false)
        .context("making the accepted host connection blocking")?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .context("setting host request timeout")?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .context("setting host response timeout")?;
    stream.set_nodelay(true).context("enabling TCP_NODELAY")?;
    let reader_stream = stream.try_clone().context("cloning host request stream")?;
    let mut reader = BufReader::new(reader_stream);
    let mut authenticated = false;
    loop {
        let mut line = String::new();
        if reader
            .read_line(&mut line)
            .context("reading host request")?
            == 0
        {
            return Ok(());
        }
        let request: HostRequest =
            serde_json::from_str(line.trim_end()).context("decoding session host request")?;
        if request.token() != state.token {
            write_json_line(
                &mut stream,
                &HostResponse::Error {
                    message: "Session host authentication failed".to_string(),
                },
            )?;
            bail!("rejected request with an invalid token");
        }
        if !authenticated {
            reader
                .get_mut()
                .set_read_timeout(None)
                .context("making authenticated host connection persistent")?;
            authenticated = true;
        }

        match request {
            HostRequest::Ping { .. } => write_json_line(
                &mut stream,
                &HostResponse::Pong {
                    protocol: SESSION_HOST_PROTOCOL,
                    pid: std::process::id(),
                },
            )?,
            HostRequest::AttachStart {
                definition,
                cols,
                rows,
                ..
            } => return attach_or_start(stream, state, definition, cols, rows),
            HostRequest::Write {
                session_id, data, ..
            } => {
                let response = write_session(state, session_id, &data).map_or_else(
                    |error| HostResponse::Error {
                        message: format!("{error:#}"),
                    },
                    |()| HostResponse::Ok,
                );
                write_json_line(&mut stream, &response)?;
            }
            HostRequest::Resize {
                session_id,
                cols,
                rows,
                ..
            } => {
                let response = resize_session(state, session_id, cols, rows).map_or_else(
                    |error| HostResponse::Error {
                        message: format!("{error:#}"),
                    },
                    |()| HostResponse::Ok,
                );
                write_json_line(&mut stream, &response)?;
            }
            HostRequest::Stop { session_id, .. } => {
                stop_session(state, session_id);
                write_json_line(&mut stream, &HostResponse::Ok)?;
            }
            HostRequest::Shutdown { .. } => {
                write_json_line(&mut stream, &HostResponse::Ok)?;
                state.shutdown.store(true, Ordering::Release);
                return Ok(());
            }
        }
    }
}

fn attach_or_start(
    mut stream: TcpStream,
    state: &Arc<HostState>,
    definition: HostSessionDefinition,
    cols: u16,
    rows: u16,
) -> Result<()> {
    let session = match get_or_start_session(state, &definition, cols, rows) {
        Ok(session) => session,
        Err(error) => {
            write_json_line(
                &mut stream,
                &HostResponse::Error {
                    message: format!("{error:#}"),
                },
            )?;
            return Ok(());
        }
    };
    let attachment_id = state.next_attachment_id.fetch_add(1, Ordering::Relaxed);
    let (sender, receiver) = mpsc::sync_channel(SUBSCRIBER_QUEUE_CAPACITY);
    let (backlog, session_state, exit_code, message) = {
        let mut runtime = session.runtime.lock();
        let snapshot = (
            runtime.backlog.iter().cloned().collect(),
            runtime.state.clone(),
            runtime.exit_code,
            runtime.message.clone(),
        );
        runtime.subscribers.insert(attachment_id, sender);
        snapshot
    };
    let response = HostResponse::Attached {
        attachment_id,
        backlog,
        state: session_state,
        exit_code,
        message,
        host_pid: std::process::id(),
    };
    if let Err(error) = write_json_line(&mut stream, &response) {
        session.runtime.lock().subscribers.remove(&attachment_id);
        return Err(error);
    }

    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let session_for_cleanup = session.clone();
    thread::Builder::new()
        .name(format!("ai-dock-host-attachment-{attachment_id}"))
        .spawn(move || {
            let mut writer = BufWriter::new(stream);
            for event in receiver {
                if write_json_line(&mut writer, &event).is_err() {
                    break;
                }
            }
            session_for_cleanup
                .runtime
                .lock()
                .subscribers
                .remove(&attachment_id);
        })
        .context("starting attachment writer")?;
    Ok(())
}

fn get_or_start_session(
    state: &Arc<HostState>,
    definition: &HostSessionDefinition,
    cols: u16,
    rows: u16,
) -> Result<Arc<HostedSession>> {
    let mut sessions = state.sessions.lock();
    if let Some(session) = sessions.get(&definition.id) {
        return Ok(session.clone());
    }

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(pty_size(cols, rows))
        .context("opening pseudoterminal in session host")?;
    let mut command = platform_command(&definition.command_line);
    if !definition.working_directory.trim().is_empty() {
        command.cwd(&definition.working_directory);
    }
    let child = pair
        .slave
        .spawn_command(command)
        .with_context(|| format!("starting {}", definition.command_line))?;
    let killer = child.clone_killer();
    let reader = pair
        .master
        .try_clone_reader()
        .context("opening hosted terminal output")?;
    let writer = pair
        .master
        .take_writer()
        .context("opening hosted terminal input")?;
    drop(pair.slave);

    let session = Arc::new(HostedSession {
        master: Mutex::new(pair.master),
        writer: Mutex::new(writer),
        killer: Mutex::new(killer),
        stop_requested: AtomicBool::new(false),
        runtime: Mutex::new(SessionRuntime {
            state: "running".to_string(),
            exit_code: None,
            message: None,
            backlog: VecDeque::new(),
            backlog_bytes: 0,
            next_output_sequence: 1,
            subscribers: HashMap::new(),
        }),
    });
    sessions.insert(definition.id, session.clone());
    drop(sessions);

    spawn_output_reader(definition.id, reader, session.clone());
    spawn_exit_watcher(definition.id, child, session.clone());
    log_host(
        &state.log_path,
        &format!("started session {}", definition.id),
    );
    Ok(session)
}

fn write_session(state: &HostState, session_id: Uuid, data: &str) -> Result<()> {
    let session = state
        .sessions
        .lock()
        .get(&session_id)
        .cloned()
        .ok_or_else(|| anyhow!("session is not running"))?;
    let mut writer = session.writer.lock();
    writer
        .write_all(data.as_bytes())
        .context("writing hosted terminal input")?;
    writer.flush().context("flushing hosted terminal input")
}

fn resize_session(state: &HostState, session_id: Uuid, cols: u16, rows: u16) -> Result<()> {
    let session = state
        .sessions
        .lock()
        .get(&session_id)
        .cloned()
        .ok_or_else(|| anyhow!("session is not running"))?;
    session
        .master
        .lock()
        .resize(pty_size(cols, rows))
        .context("resizing hosted terminal")
}

fn stop_session(state: &HostState, session_id: Uuid) {
    let session = state.sessions.lock().remove(&session_id);
    if let Some(session) = session {
        session.stop_requested.store(true, Ordering::Release);
        session.set_state(session_id, "stopped", None, None);
        // portable-pty 0.9 reverses the Windows success check and may report an error
        // after successfully terminating the process.
        let _ = session.killer.lock().kill();
        log_host(&state.log_path, &format!("stopped session {session_id}"));
    }
}

fn stop_all_sessions(state: &HostState) {
    let ids = state.sessions.lock().keys().copied().collect::<Vec<_>>();
    for session_id in ids {
        stop_session(state, session_id);
    }
}

fn spawn_output_reader(
    session_id: Uuid,
    mut reader: Box<dyn Read + Send>,
    session: Arc<HostedSession>,
) {
    let _ = thread::Builder::new()
        .name(format!("ai-dock-host-output-{session_id}"))
        .spawn(move || {
            let mut buffer = vec![0_u8; 16 * 1024];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        session.publish_output(session_id, BASE64.encode(&buffer[..count]));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        if !session.stop_requested.load(Ordering::Acquire) {
                            session.set_state(
                                session_id,
                                "failed",
                                None,
                                Some(format!("reading terminal output: {error}")),
                            );
                        }
                        break;
                    }
                }
            }
        });
}

fn spawn_exit_watcher(
    session_id: Uuid,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    session: Arc<HostedSession>,
) {
    let _ = thread::Builder::new()
        .name(format!("ai-dock-host-process-{session_id}"))
        .spawn(move || {
            let result = child.wait();
            if session.stop_requested.load(Ordering::Acquire) {
                return;
            }
            match result {
                Ok(status) => {
                    session.set_state(session_id, "exited", Some(status.exit_code()), None)
                }
                Err(error) => {
                    session.set_state(session_id, "failed", None, Some(error.to_string()))
                }
            }
        });
}

fn platform_command(command_line: &str) -> CommandBuilder {
    #[cfg(windows)]
    {
        let mut command = CommandBuilder::new("cmd.exe");
        command.arg("/d");
        command.arg("/s");
        command.arg("/c");
        command.arg(command_line);
        command
    }

    #[cfg(not(windows))]
    {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-lc");
        command.arg(command_line);
        command
    }
}

fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.clamp(2, 500),
        cols: cols.clamp(2, 500),
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn write_json_line(writer: &mut impl Write, value: &impl serde::Serialize) -> Result<()> {
    serde_json::to_writer(&mut *writer, value).context("encoding session host message")?;
    writer
        .write_all(b"\n")
        .context("writing session host message")?;
    writer.flush().context("flushing session host message")
}

fn write_json_atomically(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(value).context("encoding session host rendezvous")?;
    fs::write(&temporary, bytes).context("writing session host rendezvous")?;
    if path.exists() {
        fs::remove_file(path).context("replacing stale session host rendezvous")?;
    }
    fs::rename(&temporary, path).context("publishing session host rendezvous")
}

fn log_host(path: &Path, message: &str) {
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = writeln!(file, "{:?} {message}", std::time::SystemTime::now());
}
