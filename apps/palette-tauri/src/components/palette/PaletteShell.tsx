import type { Dispatch, RefObject, SetStateAction } from "react";

import { ActionList } from "@/components/palette/ActionList";
import { AuthNotice } from "@/components/palette/AuthNotice";
import { PaletteCommandBar } from "@/components/palette/PaletteCommandBar";
import { PaletteFooter } from "@/components/palette/PaletteFooter";
import { ResultView } from "@/components/palette/ResultView";
import { SettingsPanel } from "@/components/palette/SettingsPanel";
import type { PaletteAction } from "@/lib/actions";
import type { PaletteConfig } from "@/lib/labbyClient";
import { invoke } from "@/lib/invoke";
import type { RunState } from "@/lib/runState";

interface PaletteShellProps {
  active?: PaletteAction;
  activeDescendantId?: string;
  compact: boolean;
  config: PaletteConfig | null;
  configError: string | null;
  copied: boolean;
  draftConfig: PaletteConfig | null;
  endpointLabel: string;
  endpointTone: string;
  filtered: PaletteAction[];
  hasQuery: boolean;
  listboxOpen: boolean;
  modeAction: PaletteAction | null;
  onBack: () => void;
  onCollapse: () => void;
  onCopy: (text: string) => void;
  onEnterMode: (action: PaletteAction) => void;
  onInputKeyDown: (event: React.KeyboardEvent<HTMLInputElement>) => void;
  onQueryChange: (value: string) => void;
  onReset: () => void;
  onRetry: () => void;
  onSaveSettings: () => Promise<void>;
  onSubmitAction: (action: PaletteAction) => void;
  onToggleMaximize: () => void;
  onToggleSettings: () => void;
  query: string;
  run: RunState;
  running: boolean;
  selected: number;
  setDraftConfig: Dispatch<SetStateAction<PaletteConfig | null>>;
  setSelected: Dispatch<SetStateAction<number>>;
  settingsFocusRef: RefObject<HTMLDivElement | null>;
  settingsOpen: boolean;
  shortcutOptions: readonly string[];
  showActionPanel: boolean;
  showBackButton: boolean;
  showContent: boolean;
  showResultsLayout: boolean;
  submitDisabled: boolean;
  validation: string;
}

export function PaletteShell(props: PaletteShellProps) {
  return (
    <div
      className={`aurora-page-shell palette-shell${props.compact ? " palette-shell-compact" : ""}${props.showResultsLayout ? " palette-shell-results" : " palette-shell-browse"}`}
    >
      <AuthNotice />
      <PaletteCommandBar
        active={props.active}
        activeDescendantId={props.activeDescendantId}
        config={props.config}
        endpointLabel={props.endpointLabel}
        endpointTone={props.endpointTone}
        hasQuery={props.hasQuery}
        listboxOpen={props.listboxOpen}
        modeAction={props.modeAction}
        query={props.query}
        running={props.running}
        settingsOpen={props.settingsOpen}
        showBackButton={props.showBackButton}
        submitDisabled={props.submitDisabled}
        validation={props.validation}
        onBack={props.onBack}
        onInputKeyDown={props.onInputKeyDown}
        onQueryChange={props.onQueryChange}
        onReset={props.onReset}
        onSubmit={props.onSubmitAction}
        onToggleMaximize={props.onToggleMaximize}
        onToggleSettings={props.onToggleSettings}
      />

      {props.settingsOpen && props.draftConfig ? (
        <div ref={props.settingsFocusRef} style={{ display: "contents" }}>
          <SettingsPanel
            configError={props.configError}
            draftConfig={props.draftConfig}
            shortcutOptions={props.shortcutOptions}
            onChange={props.setDraftConfig}
            onClose={props.onBack}
            onSave={props.onSaveSettings}
          />
        </div>
      ) : null}

      {props.showContent && !props.settingsOpen ? (
        <main
          className={
            props.showResultsLayout
              ? "palette-grid palette-grid-output-only"
              : "palette-suggestions"
          }
        >
          {props.showActionPanel && (
            <ActionList
              filtered={props.filtered}
              selected={props.selected}
              setSelected={props.setSelected}
              onSubmit={props.onSubmitAction}
              onEnterMode={props.onEnterMode}
            />
          )}

          {props.showResultsLayout && (
            <ResultView
              action={props.active}
              result={"result" in props.run ? props.run.result : null}
              running={props.running}
              copied={props.copied}
              onCopy={props.onCopy}
              onRetry={props.onRetry}
              onCollapse={props.onCollapse}
            />
          )}
        </main>
      ) : null}

      {props.showContent && !props.settingsOpen ? (
        <PaletteFooter
          config={props.config}
          configError={props.configError}
          onSettings={props.onToggleSettings}
          onHide={() => void invoke("hide_palette")}
        />
      ) : null}
    </div>
  );
}
