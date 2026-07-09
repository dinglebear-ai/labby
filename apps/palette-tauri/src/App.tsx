import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { actionOptionId } from "@/components/palette/ActionList";
import { PaletteShell } from "@/components/palette/PaletteShell";
import type { PaletteAction } from "@/lib/actions";
import { useActionCatalog } from "@/lib/actionCatalog";
import { dispatchAction, resultErrorMessage } from "@/lib/labbyClient";
import { hostLabel } from "@/lib/url";
import { invoke, isTauriRuntime } from "@/lib/invoke";
import type { RunState } from "@/lib/runState";
import { usePaletteConfig } from "@/lib/usePaletteConfig";
import { usePaletteLifecycle } from "@/lib/usePaletteLifecycle";
import { useWindowChrome } from "@/lib/useWindowChrome";

const shortcutOptions = ["Ctrl+Shift+Space", "Alt+Space", "Ctrl+Space", "Cmd+Shift+Space"] as const;

document.documentElement.classList.toggle("tauri-runtime", isTauriRuntime);

function focusInput() {
  document.querySelector<HTMLInputElement>(".command-input")?.focus();
}

function exampleParams(action: PaletteAction): string {
  if (action.params.length === 0) return "{}";
  const obj: Record<string, unknown> = {};
  for (const param of action.params) {
    const ty = param.ty.toLowerCase();
    obj[param.name] = ty.includes("bool")
      ? false
      : ty.includes("int") || ty.includes("number")
        ? 0
        : ty.includes("array") || ty.includes("[")
          ? []
          : ty.includes("object") || ty.includes("map")
            ? {}
            : "";
  }
  return JSON.stringify(obj, null, 2);
}

export default function App() {
  const [mode, setMode] = useState<"browse" | "argument">("browse");
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState(0);
  const [activeAction, setActiveAction] = useState<PaletteAction | null>(null);
  const [browseOpen, setBrowseOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [run, setRun] = useState<RunState>({ kind: "idle" });
  const [copied, setCopied] = useState(false);
  const [shownTick, setShownTick] = useState(0);
  const [pendingConfirm, setPendingConfirm] = useState<string | null>(null);
  const lastParamsRef = useRef<unknown>({});
  const settingsFocusRef = useRef<HTMLDivElement | null>(null);

  const { actions: catalogActions, error: catalogError } = useActionCatalog();
  const { config, draftConfig, setDraftConfig, configError, saveSettings } = usePaletteConfig();

  usePaletteLifecycle(
    useCallback(() => setSettingsOpen(true), []),
    setShownTick,
  );

  const filtered = useMemo(() => {
    if (mode !== "browse") return [];
    const q = query.trim().toLowerCase();
    const matches = q
      ? catalogActions.filter(
          (action) =>
            action.subcommand.toLowerCase().includes(q) ||
            action.label.toLowerCase().includes(q) ||
            action.description.toLowerCase().includes(q) ||
            action.category.toLowerCase().includes(q),
        )
      : catalogActions;
    return matches.slice(0, 30);
  }, [catalogActions, query, mode]);

  useEffect(() => {
    if (selected >= filtered.length) setSelected(0);
  }, [filtered.length, selected]);

  const active = mode === "argument" ? activeAction : filtered[selected];
  const modeAction = mode === "argument" ? activeAction : null;

  const hasQuery = query.trim().length > 0;
  const showResultsLayout = run.kind !== "idle";
  const showContent = settingsOpen || showResultsLayout || mode === "argument" || hasQuery || browseOpen;
  const compact = !showContent;
  const showActionPanel = mode === "browse" && !showResultsLayout && !settingsOpen;
  const listboxOpen = showContent && showActionPanel;
  const activeDescendantId = listboxOpen && active ? actionOptionId(active) : undefined;
  const running = run.kind === "running";
  const showBackButton = settingsOpen || showResultsLayout || mode === "argument";

  useWindowChrome({
    settingsOpen,
    showResultsLayout,
    showContent,
    filteredLength: filtered.length,
    shownTick,
  });

  const argumentJson = useMemo(() => {
    if (mode !== "argument") return { ok: true as const, value: {} as unknown };
    try {
      return { ok: true as const, value: (query.trim() ? JSON.parse(query) : {}) as unknown };
    } catch {
      return { ok: false as const, value: undefined as unknown };
    }
  }, [mode, query]);

  const validation =
    mode === "argument" && !argumentJson.ok
      ? "Invalid JSON — fix and press Enter"
      : !active
        ? "No matching action"
        : pendingConfirm === active.subcommand
          ? "Press Enter again to confirm this destructive action"
          : "";

  const runAction = useCallback(async (action: PaletteAction, params: unknown) => {
    lastParamsRef.current = params;
    setRun({ kind: "running", title: action.label });
    try {
      const result = await dispatchAction(action.service, action.action, params);
      setRun(
        result.ok
          ? { kind: "success", title: action.label, result }
          : { kind: "error", title: action.label, result, message: resultErrorMessage(result) },
      );
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setRun({
        kind: "error",
        title: action.label,
        result: {
          ok: false,
          status: 0,
          path: `/v1/${action.service}`,
          method: "POST",
          payload: { error: message },
        },
        message,
      });
    }
  }, []);

  const enterArgumentMode = useCallback(
    (action: PaletteAction) => {
      if (action.params.length === 0) {
        void runAction(action, {});
        return;
      }
      setActiveAction(action);
      setQuery(exampleParams(action));
      setMode("argument");
      setPendingConfirm(null);
      focusInput();
    },
    [runAction],
  );

  const submitActive = useCallback(
    (action: PaletteAction) => {
      if (mode === "browse" && action.params.length > 0) {
        enterArgumentMode(action);
        return;
      }
      const params = mode === "argument" ? (argumentJson.ok ? argumentJson.value : undefined) : {};
      if (params === undefined) return;
      if (action.destructive && pendingConfirm !== action.subcommand) {
        setPendingConfirm(action.subcommand);
        return;
      }
      setPendingConfirm(null);
      void runAction(action, params);
    },
    [mode, argumentJson, pendingConfirm, runAction, enterArgumentMode],
  );

  const onReset = useCallback(() => {
    setQuery("");
    setSelected(0);
    setMode("browse");
    setActiveAction(null);
    setBrowseOpen(false);
    setPendingConfirm(null);
    setRun({ kind: "idle" });
  }, []);

  const onBack = useCallback(() => {
    if (settingsOpen) {
      setSettingsOpen(false);
      focusInput();
      return;
    }
    if (showResultsLayout) {
      setRun({ kind: "idle" });
      setQuery("");
      setMode("browse");
      setActiveAction(null);
      focusInput();
      return;
    }
    if (mode === "argument") {
      setMode("browse");
      setActiveAction(null);
      setQuery("");
      setPendingConfirm(null);
      focusInput();
    }
  }, [settingsOpen, showResultsLayout, mode]);

  const onCollapse = useCallback(() => {
    setRun({ kind: "idle" });
    setQuery("");
    setMode("browse");
    setActiveAction(null);
  }, []);

  const onCopy = useCallback((text: string) => {
    void navigator.clipboard.writeText(text).then(() => {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1200);
    });
  }, []);

  const onRetry = useCallback(() => {
    if (active) void runAction(active, lastParamsRef.current);
  }, [active, runAction]);

  function onInputKeyDown(event: React.KeyboardEvent<HTMLInputElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      if (mode === "argument" || settingsOpen || showResultsLayout) onBack();
      else if (hasQuery) setQuery("");
      else setBrowseOpen(false);
      return;
    }
    if (mode !== "browse") {
      if (event.key === "Enter") {
        event.preventDefault();
        if (active) submitActive(active);
      }
      return;
    }
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setBrowseOpen(true);
      setSelected((idx) => Math.min(idx + 1, Math.max(filtered.length - 1, 0)));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setSelected((idx) => Math.max(idx - 1, 0));
    } else if (event.key === "Enter") {
      event.preventDefault();
      if (active) submitActive(active);
    }
  }

  const endpointLabel = config ? hostLabel(config.serverUrl) : configError ? "Config error" : "Loading";
  const endpointTone = configError || catalogError ? "error" : "syncing";
  const submitDisabled = !active || running || Boolean(mode === "argument" && !argumentJson.ok);

  return (
    <PaletteShell
      active={active ?? undefined}
      activeDescendantId={activeDescendantId}
      compact={compact}
      config={config}
      configError={configError}
      copied={copied}
      draftConfig={draftConfig}
      endpointLabel={endpointLabel}
      endpointTone={endpointTone}
      filtered={filtered}
      hasQuery={hasQuery}
      listboxOpen={listboxOpen}
      modeAction={modeAction}
      onBack={onBack}
      onCollapse={onCollapse}
      onCopy={onCopy}
      onEnterMode={enterArgumentMode}
      onInputKeyDown={onInputKeyDown}
      onQueryChange={setQuery}
      onReset={onReset}
      onRetry={onRetry}
      onSaveSettings={saveSettings}
      onSubmitAction={submitActive}
      onToggleMaximize={() => void invoke("toggle_maximize")}
      onToggleSettings={() => setSettingsOpen((open) => !open)}
      query={query}
      run={run}
      running={running}
      selected={selected}
      setDraftConfig={setDraftConfig}
      setSelected={setSelected}
      settingsFocusRef={settingsFocusRef}
      settingsOpen={settingsOpen}
      shortcutOptions={shortcutOptions}
      showActionPanel={showActionPanel}
      showBackButton={showBackButton}
      showContent={showContent}
      showResultsLayout={showResultsLayout}
      submitDisabled={submitDisabled}
      validation={validation}
    />
  );
}
