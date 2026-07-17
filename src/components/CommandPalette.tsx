import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "cmdk";
import type { KeyboardEvent, MouseEvent } from "react";

interface CommandPaletteProps {
  open: boolean;
  onClose: () => void;
  notebooks: { id: string; name: string }[];
  activeNotebookId: string | null;
  onNewChat: () => void;
  onNewNotebook: () => void;
  onSwitchNotebook: (notebookId: string) => void;
  onOpenSettings: () => void;
  onToggleTheme: () => void;
  onImportSource: () => void;
  onViewSources: () => void;
  onViewNotes: () => void;
  onViewStudio: () => void;
}

function labelForShortcut(shortcut: string): string {
  return shortcut;
}

function shortcutText(keyboard: string, hasShift = false): string {
  const isMac = typeof navigator !== "undefined" && /Mac|iPad|iPhone|iPod/.test(navigator.platform);
  if (isMac) {
    return hasShift ? `⌘⇧${keyboard}` : `⌘${keyboard}`;
  }
  return hasShift ? `Ctrl+Shift+${keyboard}` : `Ctrl+${keyboard}`;
}

export function CommandPalette({
  open,
  onClose,
  notebooks,
  activeNotebookId,
  onNewChat,
  onNewNotebook,
  onSwitchNotebook,
  onOpenSettings,
  onToggleTheme,
  onImportSource,
  onViewSources,
  onViewNotes,
  onViewStudio,
}: CommandPaletteProps) {
  const run = (action: () => void) => {
    onClose();
    action();
  };

  const handleOutsideMouseDown = (event: MouseEvent<HTMLDivElement>) => {
    if (event.target === event.currentTarget) {
      onClose();
    }
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
    }
  };

  if (!open) return null;

  return (
    <div
      role="presentation"
      className="gloss-command-overlay"
      onMouseDown={handleOutsideMouseDown}
      onKeyDown={handleKeyDown}
    >
      <div className="gloss-command-dialog" onMouseDown={(event) => event.stopPropagation()}>
        <Command className="gloss-command-root" shouldFilter>
          <div className="gloss-command-search-row">
            <CommandInput
              autoFocus
              placeholder="Search commands and notebooks"
              className="gloss-command-input"
            />
          </div>

          <CommandList className="gloss-command-list">
            <CommandEmpty>No matching commands.</CommandEmpty>

            <CommandGroup heading="Commands">
              <CommandItem
                value="new chat create conversation command new chat cmd shift n"
                onSelect={() => run(onNewChat)}
              >
                <span className="gloss-command-label">New Chat</span>
                <span className="gloss-command-meta">{labelForShortcut(shortcutText("N", true))}</span>
              </CommandItem>

              <CommandItem
                value="new notebook create empty notebook cmd n"
                onSelect={() => run(onNewNotebook)}
              >
                <span className="gloss-command-label">New Notebook</span>
                <span className="gloss-command-meta">{labelForShortcut(shortcutText("N"))}</span>
              </CommandItem>

              <CommandItem
                value="open settings config"
                onSelect={() => run(onOpenSettings)}
              >
                <span className="gloss-command-label">Open Settings</span>
                <span className="gloss-command-meta">⌘,</span>
              </CommandItem>

              <CommandItem
                value="toggle theme dark light"
                onSelect={() => run(onToggleTheme)}
              >
                <span className="gloss-command-label">Toggle Theme</span>
                <span className="gloss-command-meta">{labelForShortcut(shortcutText("T", true))}</span>
              </CommandItem>

              <CommandItem
                value="import source paste file cmd i"
                onSelect={() => run(onImportSource)}
              >
                <span className="gloss-command-label">Import Source</span>
                <span className="gloss-command-meta">{labelForShortcut(shortcutText("I"))}</span>
              </CommandItem>

              <CommandItem
                value="view sources open sources cmd 1"
                onSelect={() => run(onViewSources)}
              >
                <span className="gloss-command-label">View Sources</span>
                <span className="gloss-command-meta">{labelForShortcut("1")}</span>
              </CommandItem>

              <CommandItem
                value="view notes open notes cmd 2"
                onSelect={() => run(onViewNotes)}
              >
                <span className="gloss-command-label">View Notes</span>
                <span className="gloss-command-meta">{labelForShortcut("2")}</span>
              </CommandItem>

              <CommandItem
                value="view studio open studio cmd 3"
                onSelect={() => run(onViewStudio)}
              >
                <span className="gloss-command-label">View Studio</span>
                <span className="gloss-command-meta">{labelForShortcut("3")}</span>
              </CommandItem>
            </CommandGroup>

            <CommandGroup heading="Switch Notebook...">
              {notebooks.length === 0 ? (
                <CommandItem
                  value="no notebooks"
                  onSelect={() => {
                    onClose();
                  }}
                  disabled
                >
                  <span className="gloss-command-label">No notebooks yet</span>
                </CommandItem>
              ) : (
                notebooks.map((notebook) => {
                  const isActive = notebook.id === activeNotebookId;
                  return (
                    <CommandItem
                      key={notebook.id}
                      value={`switch notebook ${notebook.name} ${notebook.id}`}
                      onSelect={() => {
                        onClose();
                        onSwitchNotebook(notebook.id);
                      }}
                    >
                      <span className="gloss-command-label">{notebook.name}</span>
                      {isActive ? (
                        <span className="gloss-command-meta">current</span>
                      ) : null}
                    </CommandItem>
                  );
                })
              )}
            </CommandGroup>
          </CommandList>
        </Command>
      </div>
    </div>
  );
}
