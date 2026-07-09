import { Check, Copy, RotateCw, X } from "lucide-react";

import { ErrorResultView } from "@/components/palette/ErrorResultView";
import { Button } from "@/components/ui/aurora/button";
import { Spinner } from "@/components/ui/aurora/spinner";
import type { PaletteAction } from "@/lib/actions";
import type { PaletteResult } from "@/lib/labbyClient";

interface ResultViewProps {
  action: PaletteAction | undefined;
  result: PaletteResult | null;
  running: boolean;
  copied: boolean;
  onCopy: (text: string) => void;
  onRetry: () => void;
  onCollapse: () => void;
}

// Generic result renderer: a spinner while running, ErrorResultView for the
// `{ kind, message, param, … }` envelope on failure, and pretty-printed JSON for
// a successful payload. No per-action structured views (dropped in v1).
export function ResultView({
  action,
  result,
  running,
  copied,
  onCopy,
  onRetry,
  onCollapse,
}: ResultViewProps) {
  const title = action ? action.label : "Result";
  const bodyText = result ? JSON.stringify(result.payload, null, 2) : "";

  return (
    <section className="output-panel">
      <div className="output-state">
        <header className="output-header">
          <div className="output-meta-info">
            <span className="output-title-line">
              <span className="output-title">{title}</span>
            </span>
            <span className="output-subtitle">
              {running
                ? "Running…"
                : result
                  ? `${result.method} ${result.path} → HTTP ${result.status}`
                  : "Ready"}
            </span>
          </div>
          <span className="output-tools">
            {!running && result ? (
              <>
                <Button
                  variant="plain"
                  size="unstyled"
                  type="button"
                  className={copied ? "output-tool-copied" : undefined}
                  onClick={() => onCopy(bodyText)}
                  title={copied ? "Copied" : "Copy"}
                  aria-label={copied ? "Copied output" : "Copy output"}
                >
                  {copied ? <Check size={13} /> : <Copy size={13} />}
                </Button>
                <Button
                  variant="plain"
                  size="unstyled"
                  type="button"
                  onClick={onRetry}
                  title="Re-run"
                  aria-label="Re-run action"
                >
                  <RotateCw size={13} />
                </Button>
              </>
            ) : null}
            <Button
              variant="plain"
              size="unstyled"
              type="button"
              onClick={onCollapse}
              title="Close"
              aria-label="Close output"
            >
              <X size={13} />
            </Button>
            {running ? <Spinner size="sm" /> : null}
          </span>
        </header>

        {running ? (
          <div className="output-body output-code output-pending">
            <code>Waiting for response…</code>
            <div className="output-pending-spinner">
              <Spinner size="sm" />
            </div>
          </div>
        ) : result && !result.ok ? (
          <ErrorResultView result={result} text={bodyText} />
        ) : result ? (
          <pre className="output-body output-code">
            <code>{bodyText}</code>
          </pre>
        ) : null}
      </div>
    </section>
  );
}
