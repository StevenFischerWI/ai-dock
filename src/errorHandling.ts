let errorToastTimer: number | undefined;

function errorText(error: unknown) {
  if (error instanceof Error) return error.message;
  if (typeof error === 'string') return error;
  try {
    return JSON.stringify(error);
  } catch {
    return 'Unknown error';
  }
}

export function reportUiError(context: string, error: unknown) {
  console.error(`AI Dock: ${context}`, error);
  const existing = document.querySelector<HTMLElement>('.ui-error-toast');
  const toast = existing ?? document.createElement('div');
  toast.className = 'ui-error-toast';
  toast.setAttribute('role', 'alert');
  toast.replaceChildren();

  const message = document.createElement('span');
  message.textContent = `${context}: ${errorText(error)}`;
  const dismiss = document.createElement('button');
  dismiss.type = 'button';
  dismiss.textContent = '×';
  dismiss.setAttribute('aria-label', 'Dismiss error');
  dismiss.addEventListener('click', () => toast.remove(), { once: true });
  toast.append(message, dismiss);
  if (!existing) document.body.appendChild(toast);

  window.clearTimeout(errorToastTimer);
  errorToastTimer = window.setTimeout(() => toast.remove(), 8000);
}

export function runUiAction(context: string, action: () => Promise<unknown>) {
  void Promise.resolve()
    .then(action)
    .catch((error) => reportUiError(context, error));
}

export function installGlobalErrorHandling() {
  window.addEventListener('unhandledrejection', (event) => {
    event.preventDefault();
    reportUiError('Action failed', event.reason);
  });
  window.addEventListener('error', (event) => {
    event.preventDefault();
    reportUiError('Interface error', event.error ?? event.message);
  });
}
