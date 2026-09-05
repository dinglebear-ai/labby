// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { PaletteCommandBar } from "./PaletteCommandBar";

afterEach(cleanup);

it("announces changing endpoint status outside the reset button's accessible name", () => {
  const props = {
    config: null,
    endpointLabel: "localhost",
    endpointTone: "syncing" as const,
    endpointMessage: "Loading server configuration.",
    hasQuery: false,
    listboxOpen: false,
    modeAction: null,
    query: "",
    running: false,
    settingsOpen: false,
    showBackButton: false,
    submitDisabled: true,
    validation: "",
    onBack: vi.fn(),
    onInputKeyDown: vi.fn(),
    onQueryChange: vi.fn(),
    onReset: vi.fn(),
    onSubmit: vi.fn(),
    onToggleMaximize: vi.fn(),
    onToggleSettings: vi.fn(),
  };
  const view = render(<PaletteCommandBar {...props} />);
  const status = screen.getByRole("status");
  expect(status).toHaveTextContent("Loading server configuration.");
  expect(status).toHaveAttribute("aria-live", "polite");
  expect(status).toHaveAttribute("aria-atomic", "true");
  expect(screen.getByRole("button", { name: "Reset Labby palette" })).not.toContainElement(status);
  view.rerender(
    <PaletteCommandBar
      {...props}
      endpointTone="online"
      endpointMessage="Connected to localhost."
    />,
  );
  expect(screen.getByRole("status")).toBe(status);
  expect(status).toHaveTextContent("Connected to localhost.");
});
