import { useRef, useState } from "react";
import { useNoteStore } from "../../stores/noteStore";
import { SourceViewerModal } from "../sources/SourceViewerModal";
import { StickyNote, Plus, Pin, PinOff, Trash2, Pencil, Save, X } from "lucide-react";
import type { Citation } from "../../lib/types";

interface NotesPanelProps {
  notebookId: string;
}

export function NotesPanel({ notebookId }: NotesPanelProps) {
  const { notes, loading, loadError, loadNotes, createNote, updateNote, togglePin, deleteNote } = useNoteStore();
  const mutationPending = useRef(false);
  const [saving, setSaving] = useState(false);
  const [showCreate, setShowCreate] = useState(false);
  const [newTitle, setNewTitle] = useState("");
  const [newContent, setNewContent] = useState("");
  const [editingNoteId, setEditingNoteId] = useState<string | null>(null);
  const [editTitle, setEditTitle] = useState("");
  const [editContent, setEditContent] = useState("");
  const [expandedNoteIds, setExpandedNoteIds] = useState<Set<string>>(() => new Set());
  const [deleteConfirmationId, setDeleteConfirmationId] = useState<string | null>(null);
  const [activeCitation, setActiveCitation] = useState<Citation | null>(null);

  const handleCreate = async () => {
    if (!newContent.trim() || mutationPending.current) return;
    mutationPending.current = true;
    setSaving(true);
    try {
      await createNote(notebookId, newTitle || "Untitled Note", newContent);
      setNewTitle("");
      setNewContent("");
      setShowCreate(false);
    } catch {
      // The store reports persistence failures. Keep the unsaved draft intact.
    } finally {
      mutationPending.current = false;
      setSaving(false);
    }
  };

  const startEditing = (noteId: string, title?: string, content?: string) => {
    if (mutationPending.current || editingNoteId) return;
    setDeleteConfirmationId(null);
    setEditingNoteId(noteId);
    setEditTitle(title || "");
    setEditContent(content || "");
  };

  const cancelEditing = () => {
    if (mutationPending.current) return;
    setEditingNoteId(null);
    setEditTitle("");
    setEditContent("");
  };

  const handleSaveEdit = async () => {
    if (!editingNoteId || mutationPending.current) return;
    mutationPending.current = true;
    setSaving(true);
    try {
      await updateNote(notebookId, editingNoteId, editTitle || "Untitled Note", editContent);
      setEditingNoteId(null);
      setEditTitle("");
      setEditContent("");
    } catch {
      // Preserve edits for explicit retry; the store already shows the error.
    } finally {
      mutationPending.current = false;
      setSaving(false);
    }
  };

  const handleTogglePin = async (noteId: string) => {
    if (mutationPending.current) return;
    mutationPending.current = true;
    setSaving(true);
    try {
      await togglePin(notebookId, noteId);
    } catch {
      // The store reports the failure and retains the persisted pin state.
    } finally {
      mutationPending.current = false;
      setSaving(false);
    }
  };

  const handleDelete = async (noteId: string) => {
    if (mutationPending.current || deleteConfirmationId !== noteId) return;
    mutationPending.current = true;
    setSaving(true);
    try {
      await deleteNote(notebookId, noteId);
      setDeleteConfirmationId(null);
      setExpandedNoteIds((previous) => {
        const next = new Set(previous);
        next.delete(noteId);
        return next;
      });
    } catch {
      // Keep the confirmation open for an explicit retry or cancellation.
    } finally {
      mutationPending.current = false;
      setSaving(false);
    }
  };

  const toggleExpanded = (noteId: string) => {
    setExpandedNoteIds((previous) => {
      const next = new Set(previous);
      if (next.has(noteId)) next.delete(noteId);
      else next.add(noteId);
      return next;
    });
  };

  const parseCitations = (citations?: unknown): Citation[] => {
    if (!citations) return [];
    try {
      const raw = typeof citations === "string" ? JSON.parse(citations) : citations;
      return Array.isArray(raw) ? raw : [];
    } catch {
      return [];
    }
  };

  return (
    <div className="flex h-full flex-col">
      {loading && <p role="status" className="p-2 text-xs text-text-muted">Loading notes…</p>}
      {loadError && <div role="alert" className="p-2 text-xs text-error">
        Notes could not be refreshed: {loadError}{" "}
        <button onClick={() => void loadNotes(notebookId)} disabled={saving} className="underline disabled:opacity-50">Retry</button>
      </div>}
      {saving && <p role="status" className="p-2 text-xs text-text-muted">Saving note changes…</p>}
      <div className="flex items-center justify-end border-b border-border p-2">
        <button
          onClick={() => setShowCreate(!showCreate)}
          disabled={saving}
          className="inline-flex items-center gap-1 rounded border border-border px-2 py-1 text-xs text-text-secondary hover:bg-bg-tertiary hover:text-text disabled:opacity-50"
          aria-expanded={showCreate}
          aria-controls="create-note-fields"
        >
          {showCreate ? <X className="w-4 h-4" aria-hidden="true" /> : <Plus className="w-4 h-4" aria-hidden="true" />}
          {showCreate ? "Close draft" : "New note"}
        </button>
      </div>

      {showCreate && (
        <div id="create-note-fields" className="p-2 border-b border-border space-y-1">
          <label htmlFor="new-note-title" className="block text-xs text-text-secondary">Title</label>
          <input
            id="new-note-title"
            type="text"
            value={newTitle}
            disabled={saving}
            onChange={(e) => setNewTitle(e.target.value)}
            placeholder="Title..."
            className="w-full px-2 py-1 text-xs bg-bg-tertiary border border-border rounded text-text placeholder:text-text-muted focus:outline-none focus:border-accent"
          />
          <label htmlFor="new-note-content" className="block text-xs text-text-secondary">Note</label>
          <textarea
            id="new-note-content"
            value={newContent}
            disabled={saving}
            onChange={(e) => setNewContent(e.target.value)}
            placeholder="Write your note..."
            rows={4}
            className="w-full px-2 py-1 text-xs bg-bg-tertiary border border-border rounded text-text placeholder:text-text-muted focus:outline-none focus:border-accent resize-y"
          />
          <button
            onClick={handleCreate}
            disabled={saving || !newContent.trim()}
            className="w-full py-1 text-xs bg-accent text-white rounded hover:bg-accent-hover disabled:opacity-50"
          >
            Save Note
          </button>
        </div>
      )}

      <div className="flex-1 overflow-y-auto p-1">
        {notes.map((note) => {
          const parsedCitations = parseCitations(note.citations);
          const expanded = expandedNoteIds.has(note.id);
          const noteTitle = note.title || "Untitled";

          return (
            <div
              key={note.id}
              className="px-2 py-2 rounded border-b border-border/50"
            >
              <div className="flex items-start gap-1.5">
                <StickyNote className="w-3.5 h-3.5 text-text-muted mt-0.5 shrink-0" aria-hidden="true" />
                <div className="flex-1 min-w-0">
                {editingNoteId === note.id ? (
                  <div className="space-y-1">
                    <label htmlFor={`edit-note-title-${note.id}`} className="block text-xs text-text-secondary">Title</label>
                    <input
                      id={`edit-note-title-${note.id}`}
                      type="text"
                      value={editTitle}
                      disabled={saving}
                      onChange={(e) => setEditTitle(e.target.value)}
                      className="w-full px-2 py-1 text-xs bg-bg-tertiary border border-border rounded text-text placeholder:text-text-muted focus:outline-none focus:border-accent"
                    />
                    <label htmlFor={`edit-note-content-${note.id}`} className="block text-xs text-text-secondary">Note</label>
                    <textarea
                      id={`edit-note-content-${note.id}`}
                      value={editContent}
                      disabled={saving}
                      onChange={(e) => setEditContent(e.target.value)}
                      rows={5}
                      className="w-full px-2 py-1 text-xs bg-bg-tertiary border border-border rounded text-text placeholder:text-text-muted focus:outline-none focus:border-accent resize-y"
                    />
                    <div className="flex items-center gap-1">
                      <button
                        onClick={handleSaveEdit}
                        disabled={saving}
                        aria-label={`Save changes to note ${noteTitle}`}
                        className="inline-flex items-center gap-1 px-2 py-1 text-xs bg-accent text-white rounded hover:bg-accent-hover disabled:opacity-50"
                      >
                        <Save className="w-3 h-3" aria-hidden="true" />
                        Save
                      </button>
                      <button
                        onClick={cancelEditing}
                        disabled={saving}
                        aria-label={`Cancel editing note ${noteTitle}`}
                        className="inline-flex items-center gap-1 px-2 py-1 text-xs bg-bg-tertiary text-text-secondary rounded hover:text-text disabled:opacity-50"
                      >
                        <X className="w-3 h-3" aria-hidden="true" />
                        Cancel
                      </button>
                    </div>
                  </div>
                ) : (
                  <>
                    <p className="text-xs font-medium text-text break-words">
                      {noteTitle}
                    </p>
                    <p id={`note-content-${note.id}`} className={`text-xs text-text-secondary whitespace-pre-wrap break-words mt-1 ${expanded ? "" : "line-clamp-2"}`}>
                      {expanded ? note.content : note.content.slice(0, 100)}
                    </p>
                    <button
                      onClick={() => toggleExpanded(note.id)}
                      aria-expanded={expanded}
                      aria-controls={`note-content-${note.id}`}
                      aria-label={`${expanded ? "Collapse note" : "Read full note"} ${noteTitle}`}
                      className="mt-1 rounded px-1 py-1 text-xs text-accent hover:bg-accent/10"
                    >
                      {expanded ? "Collapse note" : "Read full note"}
                    </button>
                    <div className="flex items-center gap-1 mt-1 text-xs text-text-muted">
                      <span>{note.note_type === "saved_response" ? "Saved" : "Manual"}</span>
                      {note.pinned && <span>· Pinned</span>}
                    </div>
                    {parsedCitations.length > 0 && (
                      <div className="mt-2 flex flex-wrap gap-1">
                        {parsedCitations.map((citation, index) => (
                          <button
                            key={`${note.id}-${index}`}
                            onClick={() => setActiveCitation(citation)}
                            className="rounded bg-accent/15 px-1.5 py-0.5 text-[10px] text-accent hover:bg-accent/25"
                            title={citation.quote || citation.source_title}
                          >
                            [{index + 1}] {citation.source_title}
                          </button>
                        ))}
                      </div>
                    )}
                  </>
                )}
                {editingNoteId !== note.id && (
                  <div className="mt-2 flex flex-wrap items-center gap-1">
                    <button
                      onClick={() => startEditing(note.id, note.title, note.content)}
                      disabled={saving || editingNoteId != null}
                      aria-label={`Edit note ${noteTitle}`}
                      className="inline-flex items-center gap-1 rounded border border-border px-2 py-1 text-xs text-text-secondary hover:bg-bg-tertiary disabled:opacity-50"
                    >
                      <Pencil className="w-3 h-3" aria-hidden="true" />
                      Edit
                    </button>
                    <button
                      onClick={() => void handleTogglePin(note.id)}
                      disabled={saving}
                      aria-label={`${note.pinned ? "Unpin" : "Pin"} note ${noteTitle}`}
                      className="inline-flex items-center gap-1 rounded border border-border px-2 py-1 text-xs text-text-secondary hover:bg-bg-tertiary disabled:opacity-50"
                    >
                      {note.pinned ? <PinOff className="w-3 h-3 text-accent" aria-hidden="true" /> : <Pin className="w-3 h-3" aria-hidden="true" />}
                      {note.pinned ? "Unpin" : "Pin"}
                    </button>
                    <button
                      onClick={() => setDeleteConfirmationId(note.id)}
                      disabled={saving || editingNoteId != null}
                      aria-label={`Delete note ${noteTitle}`}
                      aria-expanded={deleteConfirmationId === note.id}
                      aria-controls={`delete-note-${note.id}`}
                      className="inline-flex items-center gap-1 rounded border border-border px-2 py-1 text-xs text-text-secondary hover:bg-error/20 hover:text-error disabled:opacity-50"
                    >
                      <Trash2 className="w-3 h-3" aria-hidden="true" />
                      Delete
                    </button>
                  </div>
                )}
                {deleteConfirmationId === note.id && (
                  <div id={`delete-note-${note.id}`} role="group" aria-label={`Confirm deletion of note ${noteTitle}`} className="mt-2 space-y-2 rounded border border-error/40 bg-error/5 p-2">
                    <p className="text-xs text-text-secondary">Delete this note? This cannot be undone.</p>
                    <div className="flex flex-wrap gap-2">
                      <button onClick={() => setDeleteConfirmationId(null)} disabled={saving} className="rounded border border-border px-2 py-1 text-xs text-text hover:bg-bg-tertiary disabled:opacity-50">Cancel</button>
                      <button onClick={() => void handleDelete(note.id)} disabled={saving} className="rounded border border-error/50 px-2 py-1 text-xs text-error hover:bg-error/20 disabled:opacity-50">Confirm delete</button>
                    </div>
                  </div>
                )}
                </div>
              </div>
            </div>
          );
        })}

        {notes.length === 0 && !loading && !loadError && (
          <p className="text-xs text-text-muted text-center mt-4 px-2">
            No notes yet. Create one or save a chat response.
          </p>
        )}
      </div>

      <SourceViewerModal
        notebookId={notebookId}
        citation={activeCitation}
        open={activeCitation != null}
        onClose={() => setActiveCitation(null)}
      />
    </div>
  );
}
