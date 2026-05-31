import { useState } from "react";
import { useNotebookStore } from "../../stores/notebookStore";
import { open, save } from "@tauri-apps/plugin-dialog";
import { BookOpen, Plus, Trash2, Settings, Pencil, Save, X, Download, Upload } from "lucide-react";
import { SettingsDialog } from "../settings/SettingsDialog";
import * as api from "../../lib/tauri";
import { useToastStore } from "../../stores/toastStore";

function portableDefaultName(name: string): string {
  const safe = name.trim().replace(/[^A-Za-z0-9._-]+/g, "-").replace(/^-+|-+$/g, "");
  return `${safe || "notebook"}.glosspkg.tar.gz`;
}

export function NotebookSidebar() {
  const {
    notebooks,
    activeNotebookId,
    setActive,
    createNotebook,
    renameNotebook,
    deleteNotebook,
    loadNotebooks,
  } = useNotebookStore();
  const addToast = useToastStore((s) => s.addToast);
  const [newName, setNewName] = useState("");
  const [showCreate, setShowCreate] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [editingNotebookId, setEditingNotebookId] = useState<string | null>(null);
  const [editingName, setEditingName] = useState("");
  const [portableBusy, setPortableBusy] = useState<"export" | "import" | null>(null);

  const handleCreate = async () => {
    if (!newName.trim()) return;
    try {
      await createNotebook(newName.trim());
      setNewName("");
      setShowCreate(false);
    } catch (e) {
      console.warn('Failed to create notebook:', e);
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
      console.warn("Failed to rename notebook:", e);
    }
  };

  const handleExportNotebook = async () => {
    const notebook = notebooks.find((nb) => nb.id === activeNotebookId);
    if (!notebook || portableBusy) return;
    const packageDir = await save({
      title: "Export notebook package",
      defaultPath: portableDefaultName(notebook.name),
    });
    if (!packageDir) return;
    setPortableBusy("export");
    try {
      const receipt = await api.exportNotebookArchive(notebook.id, packageDir);
      addToast({
        type: "success",
        title: "Notebook exported",
        message: `${receipt.file_count} files archived`,
        duration: 5000,
      });
    } catch (e) {
      addToast({
        type: "error",
        title: "Notebook export failed",
        message: e instanceof Error ? e.message : String(e),
        duration: 0,
      });
    } finally {
      setPortableBusy(null);
    }
  };

  const handleImportNotebook = async () => {
    if (portableBusy) return;
    const selected = await open({
      title: "Import notebook package",
      directory: false,
      multiple: false,
      filters: [{ name: "Gloss notebook package", extensions: ["gz"] }],
    });
    if (!selected || Array.isArray(selected)) return;
    setPortableBusy("import");
    try {
      const manifest = await api.validateNotebookImportArchive(selected);
      const receipt = await api.importNotebookArchive(selected);
      await loadNotebooks();
      await setActive(receipt.imported_notebook_id);
      addToast({
        type: "success",
        title: "Notebook imported",
        message: `${manifest.notebook_name} verified`,
        duration: 5000,
      });
    } catch (e) {
      addToast({
        type: "error",
        title: "Notebook import failed",
        message: e instanceof Error ? e.message : String(e),
        duration: 0,
      });
    } finally {
      setPortableBusy(null);
    }
  };

  return (
    <div className="gloss-panel flex h-full w-56 shrink-0 flex-col border-r border-border">
      <div className="gloss-panel-header flex items-center justify-between p-3">
        <div>
          <h1 className="gloss-serif text-[17px] text-text">Notebooks</h1>
          <p className="gloss-mono text-[10px] uppercase tracking-[0.03em] text-text-muted">
            Local library
          </p>
        </div>
        <button
          onClick={() => setShowCreate(!showCreate)}
          className="rounded border border-border p-1 text-text-secondary hover:bg-bg-tertiary hover:text-text"
          title="Create notebook"
        >
          <Plus className="w-4 h-4" />
        </button>
      </div>

      {showCreate && (
        <div className="border-b border-border p-2">
          <input
            type="text"
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && handleCreate()}
            placeholder="Notebook name..."
            className="w-full rounded border border-border bg-bg-tertiary px-2 py-1 text-sm text-text placeholder:text-text-muted focus:border-accent focus:outline-none"
            aria-label="New notebook name"
            autoFocus
          />
        </div>
      )}

      <div className="flex-1 overflow-y-auto p-1.5">
        {notebooks.map((nb) => (
          <div
            key={nb.id}
            onClick={() => setActive(nb.id)}
            className={`group flex cursor-pointer items-center gap-2 rounded px-2 py-1.5 text-sm ${
              activeNotebookId === nb.id
                ? "border border-accent/35 bg-accent/15 text-accent"
                : "border border-transparent text-text-secondary hover:bg-bg-tertiary hover:text-text"
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
                className="min-w-0 flex-1 rounded border border-border bg-bg-tertiary px-1.5 py-0.5 text-xs text-text focus:border-accent focus:outline-none"
                autoFocus
              />
            ) : (
              <span className="truncate flex-1">{nb.name}</span>
            )}
            <span className="gloss-mono text-[10px] text-text-muted">{nb.source_count}</span>
            {editingNotebookId === nb.id ? (
              <>
                <button
                  onClick={(e) => { e.stopPropagation(); void handleRename(nb.id); }}
                  className="rounded p-0.5 text-text-muted hover:bg-accent/20 hover:text-accent"
                  title="Save name"
                >
                  <Save className="w-3 h-3" />
                </button>
                <button
                  onClick={(e) => { e.stopPropagation(); cancelRename(); }}
                  className="rounded p-0.5 text-text-muted hover:bg-bg-tertiary hover:text-text"
                  title="Cancel rename"
                >
                  <X className="w-3 h-3" />
                </button>
              </>
            ) : (
              <>
                <button
                  onClick={(e) => { e.stopPropagation(); startRename(nb.id, nb.name); }}
                  className="hidden rounded p-0.5 text-text-muted hover:bg-bg-tertiary hover:text-text group-hover:block"
                  title="Rename notebook"
                >
                  <Pencil className="w-3 h-3" />
                </button>
                <button
                  onClick={(e) => { e.stopPropagation(); deleteNotebook(nb.id); }}
                  className="hidden rounded p-0.5 text-text-muted hover:bg-error/20 hover:text-error group-hover:block"
                  title="Delete notebook"
                >
                  <Trash2 className="w-3 h-3" />
                </button>
              </>
            )}
          </div>
        ))}

        {notebooks.length === 0 && (
          <p className="mt-4 px-2 text-center text-xs text-text-muted">
            No notebooks yet. Click + to create one.
          </p>
        )}
      </div>

      {/* Settings footer */}
      <div className="border-t border-border p-2">
        <div className="mb-1 grid grid-cols-2 gap-1">
          <button
            onClick={() => void handleImportNotebook()}
            disabled={portableBusy !== null}
            className="flex items-center justify-center rounded border border-transparent px-2 py-1.5 text-text-secondary hover:border-border hover:bg-bg-tertiary hover:text-text disabled:cursor-not-allowed disabled:opacity-50"
            title="Import notebook package"
          >
            <Upload className="w-4 h-4" />
          </button>
          <button
            onClick={() => void handleExportNotebook()}
            disabled={!activeNotebookId || portableBusy !== null}
            className="flex items-center justify-center rounded border border-transparent px-2 py-1.5 text-text-secondary hover:border-border hover:bg-bg-tertiary hover:text-text disabled:cursor-not-allowed disabled:opacity-50"
            title="Export active notebook"
          >
            <Download className="w-4 h-4" />
          </button>
        </div>
        <button
          onClick={() => setShowSettings(true)}
          className="flex w-full items-center gap-2 rounded border border-transparent px-2 py-1.5 text-sm text-text-secondary hover:border-border hover:bg-bg-tertiary hover:text-text"
        >
          <Settings className="w-4 h-4" />
          <span>Settings</span>
        </button>
      </div>

      <SettingsDialog open={showSettings} onClose={() => setShowSettings(false)} />
    </div>
  );
}
