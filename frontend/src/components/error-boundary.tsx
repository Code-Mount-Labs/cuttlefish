import { Component, type ReactNode } from "react";
import { Button } from "./ui/button";
export class ErrorBoundary extends Component<
  { children: ReactNode },
  { failed: boolean }
> {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  render() {
    return this.state.failed ? (
      <main className="auth-page">
        <h1>Unable to display this page</h1>
        <p>Your tasks continue on the agent.</p>
        <Button onClick={() => location.reload()}>Reload</Button>
      </main>
    ) : (
      this.props.children
    );
  }
}
