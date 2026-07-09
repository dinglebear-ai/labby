import { useEffect, useRef, type Dispatch, type SetStateAction } from "react";

import { ActionIcon } from "@/components/palette/ActionIcon";
import { Button } from "@/components/ui/aurora/button";
import { Kbd } from "@/components/ui/aurora/kbd";
import { ScrollArea } from "@/components/ui/aurora/scroll-area";
import type { PaletteAction } from "@/lib/actions";

interface ActionListProps {
  filtered: PaletteAction[];
  selected: number;
  setSelected: Dispatch<SetStateAction<number>>;
  onSubmit: (action: PaletteAction) => void;
  onEnterMode: (action: PaletteAction) => void;
}

// Stable per-option id shared with the command-bar input's aria-activedescendant
// so AT announces the highlighted option as the listbox's active descendant.
export function actionOptionId(action: PaletteAction): string {
  return `action-${action.subcommand}`;
}

// The searchable, keyboard-navigable list of palette actions. A row click runs a
// no-argument action directly, otherwise it enters argument mode for that action.
export function ActionList({ filtered, selected, setSelected, onSubmit, onEnterMode }: ActionListProps) {
  const selectedRowRef = useRef<HTMLButtonElement | null>(null);
  useEffect(() => {
    selectedRowRef.current?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, []);

  // Group consecutive actions by category (they arrive category-sorted) while
  // preserving each action's absolute index for selection/keys.
  const groups: { category: string; items: { action: PaletteAction; index: number }[] }[] = [];
  filtered.forEach((action, index) => {
    const last = groups[groups.length - 1];
    if (last && last.category === action.category) {
      last.items.push({ action, index });
    } else {
      groups.push({ category: action.category, items: [{ action, index }] });
    }
  });

  return (
    <section className="action-panel">
      <div className="panel-heading">
        <span>Actions</span>
        <span className="panel-shortcuts">
          <span>
            <Kbd unstyled>tab</Kbd> args
          </span>
          <span>
            <Kbd unstyled>↵</Kbd> run
          </span>
        </span>
      </div>
      <ScrollArea className="action-scroll" viewportClassName="action-scroll-viewport">
        <div id="palette-action-list" role="listbox" aria-label="Actions" className="action-list">
          {groups.map((group) => (
            <div className="action-group" role="presentation" key={`group-${group.items[0].index}`}>
              <div className="action-section-heading" aria-hidden="true">
                <span>{group.category}</span>
              </div>
              {group.items.map(({ action, index }) => {
                const selectedRow = index === selected;
                return (
                  <div className="action-group-item" role="presentation" key={action.subcommand}>
                    <div
                      role="presentation"
                      className={selectedRow ? "action-row action-row-selected" : "action-row"}
                      onPointerEnter={() => setSelected(index)}
                    >
                      <Button
                        variant="plain"
                        size="unstyled"
                        id={actionOptionId(action)}
                        role="option"
                        aria-selected={selectedRow}
                        tabIndex={-1}
                        ref={selectedRow ? selectedRowRef : undefined}
                        className="action-row-main"
                        type="button"
                        onFocusCapture={() => setSelected(index)}
                        onClick={() => {
                          setSelected(index);
                          if (action.argMode === "none") onSubmit(action);
                          else onEnterMode(action);
                        }}
                      >
                        <ActionIcon action={action} selected={selectedRow} />
                        <span className="action-main">
                          <span className="action-title-line">
                            <span className="action-label">{action.label}</span>
                            {action.destructive ? (
                              <span className="action-async">DESTRUCTIVE</span>
                            ) : null}
                          </span>
                          <span className="action-description">{action.description}</span>
                        </span>
                      </Button>
                      <span className="action-meta" aria-hidden="true">
                        <Kbd unstyled>{action.action}</Kbd>
                      </span>
                    </div>
                  </div>
                );
              })}
            </div>
          ))}
        </div>
      </ScrollArea>
    </section>
  );
}
