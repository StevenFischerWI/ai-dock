import { Component, type ErrorInfo, type ReactNode } from 'react';
import { reportUiError } from './errorHandling';

interface Props {
  children: ReactNode;
}

interface State {
  failed: boolean;
}

export class AppErrorBoundary extends Component<Props, State> {
  state: State = { failed: false };

  static getDerivedStateFromError(): State {
    return { failed: true };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    reportUiError('Interface recovered from an error', error);
    console.error(info.componentStack);
  }

  render() {
    if (this.state.failed) {
      return (
        <main className="ui-error-fallback" role="alert">
          <span>AI Dock caught an interface error.</span>
          <button onClick={() => window.location.reload()}>Reload view</button>
        </main>
      );
    }
    return this.props.children;
  }
}
