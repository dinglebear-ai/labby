import { Activity, Check } from "lucide-react";
import { useState } from "react";

import { SettingsAuthBlock } from "@/components/palette/SettingsAuthBlock";
import { MiniToggle, SecretInput, TextInput } from "@/components/palette/SettingsFields";
import { Button } from "@/components/ui/aurora/button";
import { fetchCatalog, type PaletteConfig } from "@/lib/labbyClient";

interface SettingsPanelProps {
  configError: string | null;
  draftConfig: PaletteConfig;
  shortcutOptions: readonly string[];
  onChange: (config: PaletteConfig) => void;
  onClose: () => void;
  onSave: () => Promise<void>;
}

type ConnectionTest =
  | { status: "unknown" }
  | { status: "checking" }
  | { status: "connected"; detail: string }
  | { status: "error"; detail: string };

// Simplified settings surface for v1: connection (server URL + OAuth sign-in,
// with a static bearer token as a dev-mode fallback), client shortcut/UX
// toggles. Axon's env/config.toml tuning tabs don't apply to Labby (that's
// `labby setup`'s job, not the palette's) so this is a single screen.
export function SettingsPanel({
  configError,
  draftConfig,
  shortcutOptions,
  onChange,
  onClose,
  onSave,
}: SettingsPanelProps) {
  const [connectionTest, setConnectionTest] = useState<ConnectionTest>({ status: "unknown" });
  const [saveState, setSaveState] = useState<"idle" | "saving" | "saved" | "error">("idle");

  const updateConfig = <Key extends keyof PaletteConfig>(key: Key, value: PaletteConfig[Key]) => {
    onChange({ ...draftConfig, [key]: value });
    setConnectionTest({ status: "unknown" });
  };

  const testConnection = async () => {
    setConnectionTest({ status: "checking" });
    try {
      // Test against the *saved* config, not the unsaved draft — the bridge
      // reads settings from disk, so an unsaved server-URL edit wouldn't be
      // exercised by this otherwise. Also inside the try: a failed save (e.g.
      // a transient shortcut-registration error) must still surface here
      // instead of leaving the UI stuck on "Checking…" forever.
      await onSave();
      const result = await fetchCatalog(null);
      if (result.notModified) {
        setConnectionTest({ status: "connected", detail: "Catalog unchanged (304)." });
        return;
      }
      const services = result.catalog.services?.length ?? 0;
      const actions = result.catalog.services?.reduce((n, s) => n + (s.actions?.length ?? 0), 0) ?? 0;
      setConnectionTest({
        status: "connected",
        detail: `Connected — ${services} service${services === 1 ? "" : "s"}, ${actions} action${actions === 1 ? "" : "s"}.`,
      });
    } catch (err) {
      setConnectionTest({
        status: "error",
        detail: err instanceof Error ? err.message : String(err),
      });
    }
  };

  const handleSave = async () => {
    setSaveState("saving");
    try {
      await onSave();
      setSaveState("saved");
      window.setTimeout(() => setSaveState("idle"), 1800);
    } catch {
      setSaveState("error");
    }
  };

  return (
    <section className="settings-panel settings-panel-mock">
      <header className="settings-topline">
        <span className="settings-eyebrow">Settings</span>
      </header>

      <div className="settings-scroll">
        <div className="settings-connection-grid">
          <div className="settings-stack">
            <span className="settings-section-label">Connection</span>
            <Field
              label="Labby server"
              hint="Origin only — e.g. https://labby.tootie.tv (NOT the /mcp path)"
            >
              <TextInput
                value={draftConfig.serverUrl}
                onChange={(value) => updateConfig("serverUrl", value)}
                mono
                placeholder="http://localhost:8765"
              />
            </Field>
            <div className="settings-toggle-row">
              <span>
                <span>Test connection</span>
                {connectionTest.status === "connected" && <span>{connectionTest.detail}</span>}
                {connectionTest.status === "error" && <span>{connectionTest.detail}</span>}
                {connectionTest.status === "unknown" && (
                  <span>Saves your current settings, then calls GET /v1/catalog.</span>
                )}
              </span>
              <Button
                size="sm"
                variant="neutral"
                disabled={connectionTest.status === "checking"}
                onClick={() => void testConnection()}
              >
                <Activity size={13} />
                {connectionTest.status === "checking" ? "Checking…" : "Test"}
              </Button>
            </div>
          </div>
          <SettingsAuthBlock />
          <div className="settings-stack">
            <span className="settings-section-label">Fallback auth</span>
            <Field label="Static bearer token" hint="LABBY_MCP_HTTP_TOKEN — used when OAuth isn't signed in">
              <SecretInput
                value={draftConfig.staticToken ?? ""}
                onChange={(value) => updateConfig("staticToken", value || null)}
              />
            </Field>
          </div>
          <div className="settings-stack">
            <span className="settings-section-label">Client</span>
            <Field label="Global shortcut" hint="press to record">
              <TextInput
                value={draftConfig.shortcut || shortcutOptions[0]}
                onChange={(value) => updateConfig("shortcut", value)}
                mono
              />
            </Field>
            <ToggleRow
              label="Hide on blur"
              sub="Dismiss when the window loses focus"
              on={draftConfig.hideOnBlur}
              onChange={(value) => updateConfig("hideOnBlur", value)}
            />
            <ToggleRow
              label="Show footer hints"
              sub="Display the keyboard hint legend under the palette"
              on={draftConfig.showFooterHints ?? false}
              onChange={(value) => updateConfig("showFooterHints", value)}
            />
          </div>
        </div>
      </div>

      <footer className="settings-footer">
        <span className="settings-footer-meta">
          <Activity size={14} /> OAuth is the primary auth path; the static token is a
          dev-mode fallback.
        </span>
        {configError && <span className="settings-error">{configError}</span>}
        <div className="settings-footer-actions">
          <Button size="sm" variant="neutral" onClick={onClose}>
            Close
          </Button>
          <Button size="sm" variant="aurora" onClick={() => void handleSave()} disabled={saveState === "saving"}>
            {saveState === "saved" ? (
              <>
                <Check size={13} /> Saved
              </>
            ) : saveState === "saving" ? (
              "Saving…"
            ) : saveState === "error" ? (
              "Save failed — retry"
            ) : (
              "Save"
            )}
          </Button>
        </div>
      </footer>
    </section>
  );
}

function Field({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    // biome-ignore lint/a11y/noLabelWithoutControl: the form control is passed as `children` and rendered inside this wrapping label (implicit association)
    <label className="settings-field">
      <span className="settings-field-head">
        <span>{label}</span>
        {hint && <span>{hint}</span>}
      </span>
      {children}
    </label>
  );
}

function ToggleRow({
  label,
  sub,
  on,
  onChange,
}: {
  label: string;
  sub?: string;
  on: boolean;
  onChange: (value: boolean) => void;
}) {
  return (
    <div className="settings-toggle-row">
      <span>
        <span>{label}</span>
        {sub && <span>{sub}</span>}
      </span>
      <MiniToggle label={label} on={on} onChange={onChange} />
    </div>
  );
}
