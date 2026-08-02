import { Component, type ErrorInfo, type ReactNode } from 'react';

// ponytail: top-level error boundary. Previously any render-time throw (a
// malformed backend payload producing `undefined.field`, a bad Date parse,
// etc.) would white-screen the whole app. This catches it and shows a recovery
// UI instead. Place one at the App root and optionally around volatile subtrees
// (DownloadList, InspectorPanel) so a single component failure doesn't take
// down the shell.

interface Props {
  children: ReactNode;
  fallback?: (error: Error, reset: () => void) => ReactNode;
  /** Label emitted to the console for identifying which boundary fired. */
  label?: string;
}

interface State {
  error: Error | null;
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(`[ErrorBoundary${this.props.label ? `: ${this.props.label}` : ''}]`, error, info);
  }

  reset = () => this.setState({ error: null });

  render() {
    if (this.state.error) {
      if (this.props.fallback) {
        return this.props.fallback(this.state.error, this.reset);
      }
      return (
        <div className="err-boundary" role="alert">
          <strong>Something went wrong.</strong>
          <span className="err-detail">{this.state.error.message}</span>
          <button type="button" className="btn-primary" onClick={this.reset}>
            Try again
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}
