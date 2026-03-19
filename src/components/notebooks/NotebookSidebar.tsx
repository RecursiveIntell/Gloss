import { useState } from "react";
import { useNotebookStore } from "../../stores/notebookStore";
import { BookOpen, Plus, Trash2, Settings, Pencil, Save, X } from "lucide-react";
import { SettingsDialog } from "../settings/SettingsDialog";

export function NotebookSidebar() {
  const { notebooks, activeNotebookId, setActive, createNotebook, renameNotebook, deleteNotebook } = useNotebookStore();
  const [newName, setNewName] = useState("");
  const [showCreate, setShowCreate] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [editingNotebookId, setEditingNotebookId] = useState<string | null>(null);
  const [editingName, setEditingName] = useState("");

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

  const startRename = (id: string, name: string) => {
    setEditingNotebookId(id);
    setEditingName(name);
  };

  const cancelRename = () => {
    setEditingNotebookId(null);
    setEditingName("");
  };

  const handleRename = async (id: string) => {
    if (!editingName.trim()) return;
    try {
      await renameNotebook(id, editingName.trim());
      cancelRename();
    } catch (e) {
      console.error("Failed to rename notebook:", e);
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
            {editingNotebookId === nb.id ? (
              <input
                type="text"
                value={editingName}
                onClick={(e) => e.stopPropagation()}
                onChange={(e) => setEditingName(e.target.value)}
                onKeyDown={(e) => {
                  e.stopPropagation();
                  if (e.key === "Enter") {
                    void handleRename(nb.id);
                  } else if (e.key === "Escape") {
                    cancelRename();
                  }
                }}
                className="flex-1 min-w-0 px-1.5 py-0.5 text-xs bg-bg-tertiary border border-border rounded text-text focus:outline-none focus:border-accent"
                autoFocus
              />
            ) : (
              <span className="truncate flex-1">{nb.name}</span>
            )}
            <span className="text-xs text-text-muted">{nb.source_count}</span>
            {editingNotebookId === nb.id ? (
              <>
                <button
                  onClick={(e) => { e.stopPropagation(); void handleRename(nb.id); }}
                  className="p-0.5 rounded hover:bg-accent/20 text-text-muted hover:text-accent"
                >
                  <Save className="w-3 h-3" />
                </button>
                <button
                  onClick={(e) => { e.stopPropagation(); cancelRename(); }}
                  className="p-0.5 rounded hover:bg-bg-tertiary text-text-muted hover:text-text"
                >
                  <X className="w-3 h-3" />
                </button>
              </>
            ) : (
              <>
                <button
                  onClick={(e) => { e.stopPropagation(); startRename(nb.id, nb.name); }}
                  className="hidden group-hover:block p-0.5 rounded hover:bg-bg-tertiary text-text-muted hover:text-text"
                >
                  <Pencil className="w-3 h-3" />
                </button>
                <button
                  onClick={(e) => { e.stopPropagation(); deleteNotebook(nb.id); }}
                  className="hidden group-hover:block p-0.5 rounded hover:bg-error/20 text-text-muted hover:text-error"
                >
                  <Trash2 className="w-3 h-3" />
                </button>
              </>
            )}
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
