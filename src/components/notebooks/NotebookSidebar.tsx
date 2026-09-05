import { useRef, useState } from "react";
import { useNotebookStore } from "../../stores/notebookStore";
import { open, save } from "@tauri-apps/plugin-dialog";
import { BookOpen, Plus, Trash2, Settings, Pencil, Save, X, Download, Upload, Loader2 } from "lucide-react";
import { SettingsDialog } from "../settings/SettingsDialog";
import * as api from "../../lib/tauri";
import { useToastStore } from "../../stores/toastStore";

type NotebookStoreState = ReturnType<typeof useNotebookStore.getState>;

function portableDefaultName(name: string): string {
  const safe = name.trim().replace(/[^A-Za-z0-9._-]+/g, "-").replace(/^-+|-+$/g, "");
  return `${safe || "notebook"}.glosspkg.tar.gz`;
}

export function NotebookSidebar() {
  const notebooks = useNotebookStore((s: NotebookStoreState) => s.notebooks);
  const activeNotebookId = useNotebookStore((s: NotebookStoreState) => s.activeNotebookId);
  const setActive = useNotebookStore((s: NotebookStoreState) => s.setActive);
  const createNotebook = useNotebookStore((s: NotebookStoreState) => s.createNotebook);
  const renameNotebook = useNotebookStore((s: NotebookStoreState) => s.renameNotebook);
  const deleteNotebook = useNotebookStore((s: NotebookStoreState) => s.deleteNotebook);
  const loadNotebooks = useNotebookStore((s: NotebookStoreState) => s.loadNotebooks);
  const activationStatus = useNotebookStore((s: NotebookStoreState) => s.activationStatus);
  const activationError = useNotebookStore((s: NotebookStoreState) => s.activationError);
  const addToast = useToastStore((s) => s.addToast);
  const [newName, setNewName] = useState("");
  const [showCreate, setShowCreate] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [editingNotebookId, setEditingNotebookId] = useState<string | null>(null);
  const [editingName, setEditingName] = useState("");
  const [portableBusy, setPortableBusy] = useState<"export" | "import" | null>(null);
  const mutationPending = useRef(false);
  const [mutation, setMutation] = useState<"create" | "rename" | "delete" | null>(null);
  const [operationError, setOperationError] = useState<string | null>(null);
  const [deleteConfirmationId, setDeleteConfirmationId] = useState<string | null>(null);
  const busy = mutation !== null || portableBusy !== null || activationStatus === "pending";

  const handleCreate = async () => {
    if (!newName.trim() || mutationPending.current || portableBusy || activationStatus === "pending") return;
    mutationPending.current = true;
    setMutation("create");
    setOperationError(null);
    try {
      await createNotebook(newName.trim());
      setNewName("");
      setShowCreate(false);
    } catch (e) {
      setOperationError(`Notebook creation did not finish: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      mutationPending.current = false;
      setMutation(null);
    }
  };

  const startRename = (id: string, name: string) => {
    if (mutationPending.current || portableBusy || editingNotebookId || activationStatus === "pending") return;
    setOperationError(null);
    setDeleteConfirmationId(null);
    setEditingNotebookId(id);
    setEditingName(name);
  };

  const cancelRename = () => {
    if (mutationPending.current) return;
    setEditingNotebookId(null);
    setEditingName("");
  };

  const handleRename = async (id: string) => {
    if (!editingName.trim() || id !== editingNotebookId || mutationPending.current || portableBusy || activationStatus === "pending") return;
    mutationPending.current = true;
    setMutation("rename");
    setOperationError(null);
    try {
      await renameNotebook(id, editingName.trim());
      setEditingNotebookId(null);
      setEditingName("");
    } catch (e) {
      setOperationError(`Could not rename notebook: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      mutationPending.current = false;
      setMutation(null);
    }
  };

  const handleDelete = async (id: string) => {
    if (deleteConfirmationId !== id || mutationPending.current || portableBusy || activationStatus === "pending") return;
    const notebook = notebooks.find((candidate) => candidate.id === id);
    if (!notebook) return;
    mutationPending.current = true;
    setMutation("delete");
    setOperationError(null);
    try {
      await deleteNotebook(id);
      setDeleteConfirmationId(null);
    } catch (e) {
      setOperationError(`Could not delete “${notebook.name}”: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      mutationPending.current = false;
      setMutation(null);
    }
  };

  const handleActivate = async (id: string) => {
    if (busy) return;
    setOperationError(null);
    setDeleteConfirmationId(null);
    try {
      await setActive(id);
    } catch (error) {
      setOperationError(`Could not open notebook: ${error instanceof Error ? error.message : String(error)}`);
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
          onClick={() => { setDeleteConfirmationId(null); setOperationError(null); setShowCreate(!showCreate); }}
          disabled={busy}
          aria-label={showCreate ? "Close notebook draft" : "Create notebook"}
          aria-expanded={showCreate}
          className="rounded border border-border p-1 text-text-secondary hover:bg-bg-tertiary hover:text-text"
          title="Create notebook"
        >
          <Plus className="w-4 h-4" />
        </button>
      </div>

      {(operationError || activationError) && <p role="alert" className="break-words border-b border-border px-3 py-2 text-xs text-error">{operationError || `Could not activate notebook: ${activationError}`}</p>}
      {mutation && <p role="status" className="px-3 py-2 text-xs text-text-muted">{mutation === "create" ? "Creating notebook…" : mutation === "rename" ? "Saving notebook name…" : "Deleting notebook…"}</p>}

      {showCreate && (
        <div className="border-b border-border p-2">
          <input
            type="text"
            value={newName}
            disabled={busy}
            onChange={(e) => setNewName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.nativeEvent.isComposing && e.keyCode !== 229) {
                e.preventDefault();
                void handleCreate();
              }
            }}
            placeholder="Notebook name..."
            className="w-full rounded border border-border bg-bg-tertiary px-2 py-1 text-sm text-text placeholder:text-text-muted focus:border-accent focus:outline-none"
            aria-label="New notebook name"
            autoFocus
          />
          <button onClick={() => void handleCreate()} disabled={busy || !newName.trim()}
            className="mt-2 w-full rounded bg-accent px-2 py-1 text-xs text-white disabled:opacity-50">
            {mutation === "create" ? "Creating…" : "Create notebook"}
          </button>
        </div>
      )}

      <div className="relative flex-1 overflow-y-auto p-1.5">
        {/* D14 — "Switching notebook..." overlay during activation. The
            activationStatus flag is set to 'pending' in setActive() and
            cleared to 'confirmed' once the backend confirms. While pending,
            a backdrop-blur overlay prevents misclicks and shows a spinner. */}
        {activationStatus === "pending" && (
          <div
            className="pointer-events-auto absolute inset-0 z-10 flex items-center justify-center bg-black/40 backdrop-blur-sm"
            data-testid="notebook-switch-overlay"
          >
            <div className="flex items-center gap-2 rounded border border-border bg-bg-secondary px-3 py-2 text-xs text-text">
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
              <span>Switching notebook…</span>
            </div>
          </div>
        )}
        {notebooks.map((nb) => (
          <div
            key={nb.id}
            className={`group rounded px-2 py-1.5 text-sm ${
              activeNotebookId === nb.id
                ? "border border-accent/35 bg-accent/15 text-accent"
                : "border border-transparent text-text-secondary hover:bg-bg-tertiary hover:text-text"
            }`}
          >
            <div className="flex items-center gap-2">
            <BookOpen className="w-4 h-4 shrink-0" />
            {editingNotebookId === nb.id ? (
              <input
                type="text"
                value={editingName}
                disabled={busy}
                aria-label={`Rename notebook ${nb.name}`}
                onClick={(e) => e.stopPropagation()}
                onChange={(e) => setEditingName(e.target.value)}
                onKeyDown={(e) => {
                  e.stopPropagation();
                  if (e.key === "Enter" && !e.nativeEvent.isComposing && e.keyCode !== 229) {
                    e.preventDefault();
                    void handleRename(nb.id);
                  } else if (e.key === "Escape") {
                    cancelRename();
                  }
                }}
                className="min-w-0 flex-1 rounded border border-border bg-bg-tertiary px-1.5 py-0.5 text-xs text-text focus:border-accent focus:outline-none"
                autoFocus
              />
            ) : (
              <button className="min-w-0 flex-1 truncate text-left disabled:opacity-50" disabled={busy}
                aria-current={activeNotebookId === nb.id ? "page" : undefined}
                onClick={() => void handleActivate(nb.id)}>{nb.name}</button>
            )}
            <span className="gloss-mono text-[10px] text-text-muted">{nb.source_count}</span>
            {editingNotebookId === nb.id ? (
              <>
                <button
                  onClick={(e) => { e.stopPropagation(); void handleRename(nb.id); }}
                  disabled={busy || !editingName.trim()}
                  aria-label={`Save notebook name ${nb.name}`}
                  className="rounded p-0.5 text-text-muted hover:bg-accent/20 hover:text-accent"
                  title="Save name"
                >
                  <Save className="w-3 h-3" />
                </button>
                <button
                  onClick={(e) => { e.stopPropagation(); cancelRename(); }}
                  disabled={busy}
                  aria-label="Cancel notebook rename"
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
                  disabled={busy || editingNotebookId !== null}
                  aria-label={`Rename notebook ${nb.name}`}
                  className="rounded p-0.5 text-text-muted hover:bg-bg-tertiary hover:text-text disabled:opacity-50"
                  title="Rename notebook"
                >
                  <Pencil className="w-3 h-3" />
                </button>
                <button
                  onClick={() => { setOperationError(null); setDeleteConfirmationId(nb.id); }}
                  disabled={busy || editingNotebookId !== null}
                  aria-label={`Delete notebook ${nb.name}`}
                  aria-expanded={deleteConfirmationId === nb.id}
                  className="rounded p-0.5 text-text-muted hover:bg-error/20 hover:text-error disabled:opacity-50"
                  title="Delete notebook"
                >
                  <Trash2 className="w-3 h-3" />
                </button>
              </>
            )}
            </div>
            {deleteConfirmationId === nb.id && <NotebookDeleteConfirmation name={nb.name} sourceCount={nb.source_count}
              pending={mutation === "delete"}
              disabled={busy}
              onCancel={() => { if (!mutationPending.current) setDeleteConfirmationId(null); }}
              onConfirm={() => void handleDelete(nb.id)} />}
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
            disabled={busy}
            className="flex items-center justify-center rounded border border-transparent px-2 py-1.5 text-text-secondary hover:border-border hover:bg-bg-tertiary hover:text-text disabled:cursor-not-allowed disabled:opacity-50"
            title="Import notebook package"
          >
            <Upload className="w-4 h-4" />
          </button>
          <button
            onClick={() => void handleExportNotebook()}
            disabled={!activeNotebookId || busy}
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

export function NotebookDeleteConfirmation({ name, sourceCount, pending, disabled = pending, onCancel, onConfirm }: {
  name: string; sourceCount: number; pending: boolean; disabled?: boolean; onCancel: () => void; onConfirm: () => void;
}) {
  return <div role="group" aria-label={`Confirm deletion of notebook ${name}`} className="mt-2 space-y-2 rounded border border-error/40 bg-error/5 p-2"
    onKeyDown={(event) => { if (event.key === "Escape" && !disabled) { event.preventDefault(); onCancel(); } }}>
    <p className="break-words text-xs text-text-secondary">Delete “{name}” and its {sourceCount} sources, chats and notes? This cannot be undone. Export it first if you need a backup.</p>
    <div className="flex flex-wrap gap-2">
      <button type="button" onClick={onCancel} disabled={disabled} autoFocus aria-label="Cancel notebook deletion"
        className="rounded border border-border px-2 py-1 text-xs text-text-secondary disabled:opacity-50">Cancel</button>
      <button type="button" onClick={onConfirm} disabled={disabled} aria-label={`Confirm delete notebook ${name}`}
        className="rounded border border-error/40 px-2 py-1 text-xs text-error disabled:opacity-50">{pending ? "Deleting…" : "Delete notebook"}</button>
    </div>
  </div>;
}
