import { useState } from "react";
import { useNotebookStore } from "../../stores/notebookStore";
import { BookOpen, Plus, Trash2, Settings } from "lucide-react";
import { SettingsDialog } from "../settings/SettingsDialog";

export function NotebookSidebar() {
  const { notebooks, activeNotebookId, setActive, createNotebook, deleteNotebook } = useNotebookStore();
  const [newName, setNewName] = useState("");
  const [showCreate, setShowCreate] = useState(false);
  const [showSettings, setShowSettings] = useState(false);

  const handleCreate = async () => {
    if (!newName.trim()) return;
    try {
      await createNotebook(newName.trim());
      setNewName("");
      setShowCreate(false);
    } catch (e) {
      console.error('Failed to create notebook:', e);
    }
  };

  return (
    <div className="w-56 bg-bg-secondary border-r border-border flex flex-col h-full">
      <div className="p-3 border-b border-border flex items-center justify-between">
        <h1 className="text-sm font-semibold text-text">Notebooks</h1>
        <button
          onClick={() => setShowCreate(!showCreate)}
          className="p-1 rounded hover:bg-bg-tertiary text-text-secondary hover:text-text"
        >
          <Plus className="w-4 h-4" />
        </button>
      </div>

      {showCreate && (
        <div className="p-2 border-b border-border">
          <input
            type="text"
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && handleCreate()}
            placeholder="Notebook name..."
            className="w-full px-2 py-1 text-sm bg-bg-tertiary border border-border rounded text-text placeholder:text-text-muted focus:outline-none focus:border-accent"
            autoFocus
          />
        </div>
      )}

      <div className="flex-1 overflow-y-auto p-1">
        {notebooks.map((nb) => (
          <div
            key={nb.id}
            onClick={() => setActive(nb.id)}
            className={`group flex items-center gap-2 px-2 py-1.5 rounded cursor-pointer text-sm ${
              activeNotebookId === nb.id
                ? "bg-accent/10 text-accent"
                : "text-text-secondary hover:bg-bg-tertiary hover:text-text"
            }`}
          >
            <BookOpen className="w-4 h-4 shrink-0" />
            <span className="truncate flex-1">{nb.name}</span>
            <span className="text-xs text-text-muted">{nb.source_count}</span>
            <button
              onClick={(e) => { e.stopPropagation(); deleteNotebook(nb.id); }}
              className="hidden group-hover:block p-0.5 rounded hover:bg-error/20 text-text-muted hover:text-error"
            >
              <Trash2 className="w-3 h-3" />
            </button>
          </div>
        ))}

        {notebooks.length === 0 && (
          <p className="text-xs text-text-muted text-center mt-4 px-2">
            No notebooks yet. Click + to create one.
          </p>
        )}
      </div>

      {/* Settings footer */}
      <div className="border-t border-border p-2">
        <button
          onClick={() => setShowSettings(true)}
          className="flex items-center gap-2 w-full px-2 py-1.5 rounded text-sm text-text-secondary hover:bg-bg-tertiary hover:text-text"
        >
          <Settings className="w-4 h-4" />
          <span>Settings</span>
        </button>
      </div>

      <SettingsDialog open={showSettings} onClose={() => setShowSettings(false)} />
    </div>
  );
}
