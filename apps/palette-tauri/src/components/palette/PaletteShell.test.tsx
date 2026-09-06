import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("@/components/palette/AuthNotice", () => ({ AuthNotice: () => null }));
vi.mock("@/components/palette/PaletteCommandBar", () => ({ PaletteCommandBar: () => null }));
vi.mock("@/components/palette/PaletteFooter", () => ({ PaletteFooter: () => null }));
vi.mock("@/components/palette/ActionList", () => ({ ActionList: () => <div>results</div> }));

import { PaletteShell } from "./PaletteShell";

function renderShell(partialResults: boolean) {
  return render(
    <PaletteShell
      compact={false}
      config={null}
      configError={null}
      copied={false}
      draftConfig={null}
      endpointLabel="Labby"
      endpointTone="online"
      endpointMessage="Connected"
      filtered={[]}
      hasQuery
      listboxOpen
      modeAction={null}
      partialResults={partialResults}
      onBack={vi.fn()}
      onCollapse={vi.fn()}
      onCopy={vi.fn()}
      onEnterMode={vi.fn()}
      onInputKeyDown={vi.fn()}
      onQueryChange={vi.fn()}
      onReset={vi.fn()}
      onRetry={vi.fn()}
      onSaveSettings={vi.fn()}
      onSubmitAction={vi.fn()}
      onToggleMaximize={vi.fn()}
      onToggleSettings={vi.fn()}
      query="needle"
      run={{ kind: "idle" }}
      running={false}
      selected={0}
      setDraftConfig={vi.fn()}
      setSelected={vi.fn()}
      settingsFocusRef={{ current: null }}
      settingsOpen={false}
      shortcutOptions={[]}
      showActionPanel
      showBackButton={false}
      showContent
      showResultsLayout={false}
      submitDisabled
      validation=""
    />,
  );
}

describe("PaletteShell partial results", () => {
  it("shows and clears the partial-results status without hiding entries", () => {
    const view = renderShell(true);
    expect(screen.getByRole("status")).toHaveTextContent("Partial results");
    expect(screen.getByText("results")).toBeInTheDocument();

    view.unmount();
    renderShell(false);
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    expect(screen.getByText("results")).toBeInTheDocument();
  });
});
