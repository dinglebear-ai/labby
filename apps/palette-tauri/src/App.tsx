import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { actionOptionId } from "@/components/palette/ActionList";
import { PaletteShell } from "@/components/palette/PaletteShell";
import { launcherEntryMatches, type LauncherEntry, useLauncherCatalog } from "@/lib/launcherCatalog";
import { executeLauncherEntry, resultErrorMessage } from "@/lib/labbyClient";
import { exampleLauncherParams, validateLauncherParams } from "@/lib/launcherValidation";
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

export default function App() {
  const [mode, setMode] = useState<"browse" | "argument">("browse");
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState(0);
  const [activeAction, setActiveAction] = useState<LauncherEntry | null>(null);
  const [browseOpen, setBrowseOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [run, setRun] = useState<RunState>({ kind: "idle" });
  const [copied, setCopied] = useState(false);
  const [shownTick, setShownTick] = useState(0);
  const [pendingConfirm, setPendingConfirm] = useState<string | null>(null);
  const lastParamsRef = useRef<unknown>({});
  const runRequestIdRef = useRef(0);
  const settingsFocusRef = useRef<HTMLDivElement | null>(null);

  const { actions: catalogActions, error: catalogError } = useLauncherCatalog();
  const { config, draftConfig, setDraftConfig, configError, saveSettings } = usePaletteConfig();

  usePaletteLifecycle(
    useCallback(() => setSettingsOpen(true), []),
    setShownTick,
  );

  const filtered = useMemo(() => {
    if (mode !== "browse") return [];
    const matches = catalogActions.filter((action) => launcherEntryMatches(action, query));
    return matches.slice(0, 30);
  }, [catalogActions, query, mode]);

  useEffect(() => {
    if (selected >= filtered.length) setSelected(0);
  }, [filtered.length, selected]);

  const active = mode === "argument" ? activeAction : filtered[selected];
  const modeAction = mode === "argument" ? activeAction : null;

  // A destructive action's confirmation arms on the first Enter and must
  // require a second, deliberate Enter — but `pendingConfirm` alone can't
  // tell "user pressed Enter again on the same row" apart from "user arrowed
  // away and back (or just hovered another row) and this row happens to be
  // selected again." Clearing it on every active-action change means the arm
  // never survives a navigation away, so it can only fire on two consecutive
  // Enters with no selection change in between.
  useEffect(() => {
    setPendingConfirm((current) => (current && active?.id !== current ? null : current));
  }, [active?.id]);

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
      : mode === "argument" && active && !validateLauncherParams(active, argumentJson.value).valid
        ? (validateLauncherParams(active, argumentJson.value).message ?? "Params do not match schema")
      : !active
        ? "No matching action"
        : pendingConfirm === active.id
          ? "Press Enter again to confirm this destructive action"
          : "";

  const runAction = useCallback(async (action: LauncherEntry, params: unknown) => {
    const requestId = runRequestIdRef.current + 1;
    runRequestIdRef.current = requestId;
    lastParamsRef.current = params;
    setRun({ kind: "running", title: action.label });
    try {
      const result = await executeLauncherEntry(action.id, params, { confirmDestructive: action.destructive });
      if (runRequestIdRef.current !== requestId) return;
      setRun(
        result.ok
          ? { kind: "success", title: action.label, result }
          : { kind: "error", title: action.label, result, message: resultErrorMessage(result) },
      );
    } catch (err) {
      if (runRequestIdRef.current !== requestId) return;
      const message = err instanceof Error ? err.message : String(err);
      setRun({
        kind: "error",
        title: action.label,
        result: {
          ok: false,
          status: 0,
          path: "/v1/palette/execute",
          method: "POST",
          payload: { error: message },
        },
        message,
      });
    }
  }, []);

  const enterArgumentMode = useCallback(
    (action: LauncherEntry) => {
      if (action.argMode === "none") {
        void runAction(action, {});
        return;
      }
      setActiveAction(action);
      setQuery(exampleLauncherParams(action));
      setMode("argument");
      setPendingConfirm(null);
      focusInput();
    },
    [runAction],
  );

  const submitActive = useCallback(
    (action: LauncherEntry) => {
      if (mode === "browse" && action.argMode !== "none") {
        enterArgumentMode(action);
        return;
      }
      const params = mode === "argument" ? (argumentJson.ok ? argumentJson.value : undefined) : {};
      if (params === undefined) return;
      const paramValidation = validateLauncherParams(action, params);
      if (!paramValidation.valid) return;
      if (action.destructive && pendingConfirm !== action.id) {
        setPendingConfirm(action.id);
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
