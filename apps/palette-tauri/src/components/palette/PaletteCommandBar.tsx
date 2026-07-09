import { ArrowLeft, CircleHelp, Search, Send, Settings } from "lucide-react";

import { actionIcon } from "@/components/palette/ActionIcon";
import { Button } from "@/components/ui/aurora/button";
import { Input } from "@/components/ui/aurora/input";
import type { PaletteAction } from "@/lib/actions";
import type { PaletteConfig } from "@/lib/labbyClient";
import { argumentPlaceholder, focusInput } from "@/lib/paletteView";

interface PaletteCommandBarProps {
  active?: PaletteAction;
  activeDescendantId?: string;
  config: PaletteConfig | null;
  endpointLabel: string;
  endpointTone: string;
  hasQuery: boolean;
  listboxOpen: boolean;
  modeAction: PaletteAction | null;
  query: string;
  running: boolean;
  settingsOpen: boolean;
  showBackButton: boolean;
  submitDisabled: boolean;
  validation: string;
  onBack: () => void;
  onInputKeyDown: React.KeyboardEventHandler<HTMLInputElement>;
  onQueryChange: (value: string) => void;
  onReset: () => void;
  onSubmit: (action: PaletteAction) => void;
  onToggleMaximize: () => void;
  onToggleSettings: () => void;
}

function endpointStatusLabel(endpointLabel: string): string {
  return `Server: ${endpointLabel}`;
}

export function PaletteCommandBar({
  active,
  activeDescendantId,
  config,
  endpointLabel,
  endpointTone,
  hasQuery,
  listboxOpen,
  modeAction,
  query,
  running,
  settingsOpen,
  showBackButton,
  submitDisabled,
  validation,
  onBack,
  onInputKeyDown,
  onQueryChange,
  onReset,
  onSubmit,
  onToggleMaximize,
  onToggleSettings,
}: PaletteCommandBarProps) {
  const ModeIcon = modeAction ? actionIcon(modeAction.category) : null;
  const validationId = "command-validation";

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: command-bar is a layout container; double-click toggles window chrome, not an interactive widget
    <section
      className="command-bar"
      onDoubleClick={(event) => {
        if ((event.target as HTMLElement).closest("input, button, a")) return;
        onToggleMaximize();
      }}
    >
      {showBackButton && (
        <Button
          variant="plain"
          size="unstyled"
          className="command-back"
          type="button"
          onClick={onBack}
          aria-label="Back"
          title="Back"
        >
          <ArrowLeft size={17} />
        </Button>
      )}
      <Button
        variant="plain"
        size="unstyled"
        className="axon-brand"
        type="button"
        onClick={onReset}
        title={config?.serverUrl ?? endpointLabel}
        aria-label="Reset Labby palette"
      >
        <span className="axon-word">Labby</span>
        <span className={`axon-status-dot axon-status-${endpointTone}`}>
          <span className="sr-only">{endpointStatusLabel(endpointLabel)}</span>
        </span>
      </Button>
      <span className="axon-divider" aria-hidden="true" />
      {/* biome-ignore lint/a11y/noStaticElementInteractions: click-to-focus convenience; the real control is the command input within */}
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: keyboard users focus the input directly; this wrapper only expands the pointer target */}
      <div className="command-input-wrap" onClick={() => focusInput()}>
        {modeAction && ModeIcon ? (
          <span className="command-mode-icon" aria-hidden="true">
            <ModeIcon size={15} strokeWidth={1.9} />
          </span>
        ) : (
          <Search size={16} strokeWidth={1.65} aria-hidden="true" />
        )}
        <Input
          unstyled
          value={query}
          onChange={(event) => onQueryChange(event.target.value)}
          onKeyDown={onInputKeyDown}
          placeholder={
            modeAction
              ? argumentPlaceholder(modeAction)
              : hasQuery
                ? "Search commands"
                : "Search or run a Labby action…"
          }
          className="command-input"
          role="combobox"
          aria-expanded={listboxOpen}
          aria-controls={listboxOpen ? "palette-action-list" : undefined}
          aria-activedescendant={listboxOpen ? activeDescendantId : undefined}
          aria-autocomplete="list"
          aria-describedby={validation ? validationId : undefined}
          aria-label={modeAction ? `${modeAction.label} argument` : "Labby command"}
        />
        {validation && (
          <span id={validationId} className="sr-only" role="status">
            {validation}
          </span>
        )}
      </div>
      <Button
        variant="plain"
        size="unstyled"
        className={`${active && !validation ? "command-submit command-submit-armed" : "command-submit"} disabled:opacity-100`}
        type="button"
        onClick={() => active && onSubmit(active)}
        disabled={submitDisabled}
        aria-label="Run selected action"
        title={validation || "Run selected action"}
      >
        <Send size={15} />
      </Button>
      <Button
        variant="plain"
        size="unstyled"
        className={settingsOpen ? "command-settings command-settings-active" : "command-settings"}
        type="button"
        onClick={onToggleSettings}
        aria-label="Settings"
        title="Settings"
        disabled={running}
      >
        {settingsOpen ? <CircleHelp size={15} /> : <Settings size={15} />}
      </Button>
    </section>
  );
}
