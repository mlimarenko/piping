import { Component, type ErrorInfo, type ReactNode } from "react";
import { AlertTriangle } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import { captureException } from "@/shared/lib/observability";

interface ErrorBoundaryFallbackProps {
  error: Error;
  reset: () => void;
  scope?: string;
}

interface Props {
  /** Optional label attached to Sentry events, e.g. a feature or app area name. */
  scope?: string;
  /** Custom fallback. A render callback receives the retry handler. */
  fallback?: ReactNode | ((props: ErrorBoundaryFallbackProps) => ReactNode);
  /**
   * Clears the caught error whenever this value changes.
   *
   * Without it the only recovery was the retry button, which re-rendered the
   * same route that had just thrown — so navigating away left the fallback on
   * screen. Pass the route path to make navigation a recovery.
   */
  resetKey?: string;
  children: ReactNode;
}

interface State {
  error: Error | null;
}

export class ErrorBoundary extends Component<Props, State> {
  override state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  override componentDidCatch(error: Error, info: ErrorInfo): void {
    captureException(error, {
      tags: this.props.scope ? { errorBoundary: this.props.scope } : {},
      extras: { componentStack: info.componentStack },
    });
  }

  /** Navigating away is a recovery: drop the caught error when the key changes. */
  override componentDidUpdate(previous: Props): void {
    if (this.state.error !== null && previous.resetKey !== this.props.resetKey) {
      this.setState({ error: null });
    }
  }

  private readonly reset = (): void => {
    this.setState({ error: null });
  };

  override render(): ReactNode {
    const { error } = this.state;
    if (!error) return this.props.children;

    const { fallback } = this.props;
    if (typeof fallback === "function") {
      return fallback({ error, reset: this.reset, scope: this.props.scope });
    }
    if (fallback !== undefined) return fallback;

    return (
      <div className="flex min-h-[50vh] items-center justify-center p-6">
        <div role="alert" className="w-full max-w-sm rounded-lg border bg-card p-6 text-center shadow-sm">
          <AlertTriangle className="mx-auto mb-3 h-6 w-6 text-status-negative" />
          <h2 className="text-base font-semibold text-foreground">Something went wrong</h2>
          <p className="mt-2 text-sm text-muted-foreground">
            The view failed to load. Retry to render it again.
          </p>
          <Button className="mt-5" onClick={this.reset}>
            Retry
          </Button>
        </div>
      </div>
    );
  }
}
