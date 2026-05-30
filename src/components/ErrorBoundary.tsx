import { Component, type ReactNode } from "react";

interface ErrorBoundaryProps {
  children: ReactNode;
  fallback?: ReactNode;
}

interface ErrorBoundaryState {
  hasError: boolean;
  error: Error | null;
}

export class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  constructor(props: ErrorBoundaryProps) {
    super(props);
    this.state = { hasError: false, error: null };
  }

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { hasError: true, error };
  }

  componentDidCatch(error: Error, errorInfo: React.ErrorInfo): void {
    console.warn("[ErrorBoundary] Caught React error:", error, errorInfo);
  }

  handleReset = () => {
    this.setState({ hasError: false, error: null });
  };

  render(): ReactNode {
    if (this.state.hasError) {
      if (this.props.fallback) {
        return this.props.fallback;
      }
      return (
        <div className="flex h-screen items-center justify-center bg-bg text-text">
          <div className="max-w-md text-center">
            <h1 className="mb-2 text-2xl font-semibold text-red-500">
              Something went wrong
            </h1>
            <p className="mb-4 text-sm text-text-muted">
              {this.state.error?.message ?? "An unexpected error occurred."}
            </p>
            <button
              className="rounded bg-accent px-4 py-2 text-sm font-medium text-white hover:opacity-90"
              onClick={this.handleReset}
            >
              Try again
            </button>
          </div>
        </div>
      );
    }

    return this.props.children;
  }
}