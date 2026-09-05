import { useState } from "react";
import { useNoteStore } from "../../stores/noteStore";
import { SourceViewerModal } from "../sources/SourceViewerModal";
import { StickyNote, Plus, Pin, PinOff, Trash2, Pencil, Save, X } from "lucide-react";
import type { Citation } from "../../lib/types";

interface NotesPanelProps {
  notebookId: string;
}

export function NotesPanel({ notebookId }: NotesPanelProps) {
  const { notes, loading, loadError, loadNotes, createNote, updateNote, togglePin, deleteNote } = useNoteStore();
  const [saving, setSaving] = useState(false);
  const [showCreate, setShowCreate] = useState(false);
  const [newTitle, setNewTitle] = useState("");
  const [newContent, setNewContent] = useState("");
  const [editingNoteId, setEditingNoteId] = useState<string | null>(null);
  const [editTitle, setEditTitle] = useState("");
  const [editContent, setEditContent] = useState("");
  const [activeCitation, setActiveCitation] = useState<Citation | null>(null);

  const handleCreate = async () => {
    if (!newContent.trim() || saving) return;
    setSaving(true);
    try {
      await createNote(notebookId, newTitle || "Untitled Note", newContent);
      setNewTitle("");
      setNewContent("");
      setShowCreate(false);
    } catch {
      // The store reports persistence failures. Keep the unsaved draft intact.
    } finally { setSaving(false); }
  };

  const startEditing = (noteId: string, title?: string, content?: string) => {
    setEditingNoteId(noteId);
    setEditTitle(title || "");
    setEditContent(content || "");
  };

  const cancelEditing = () => {
    setEditingNoteId(null);
    setEditTitle("");
    setEditContent("");
  };

  const handleSaveEdit = async () => {
    if (!editingNoteId || saving) return;
    setSaving(true);
    try {
      await updateNote(notebookId, editingNoteId, editTitle || "Untitled Note", editContent);
      cancelEditing();
    } catch {
      // Preserve edits for explicit retry; the store already shows the error.
    } finally { setSaving(false); }
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
        <button onClick={() => void loadNotes(notebookId)} className="underline">Retry</button>
      </div>}
      <div className="flex items-center justify-end border-b border-border p-2">
        <button
          onClick={() => setShowCreate(!showCreate)}
          disabled={saving}
          className="rounded border border-border p-1 text-text-secondary hover:bg-bg-tertiary hover:text-text"
          title="Create note"
        >
          <Plus className="w-4 h-4" />
        </button>
      </div>

      {showCreate && (
        <div className="p-2 border-b border-border space-y-1">
          <input
            type="text"
            value={newTitle}
            disabled={saving}
            onChange={(e) => setNewTitle(e.target.value)}
            placeholder="Title..."
            className="w-full px-2 py-1 text-xs bg-bg-tertiary border border-border rounded text-text placeholder:text-text-muted focus:outline-none focus:border-accent"
          />
          <textarea
            value={newContent}
            disabled={saving}
            onChange={(e) => setNewContent(e.target.value)}
            placeholder="Write your note..."
            rows={4}
            className="w-full px-2 py-1 text-xs bg-bg-tertiary border border-border rounded text-text placeholder:text-text-muted focus:outline-none focus:border-accent resize-none"
          />
          <button
            onClick={handleCreate}
            disabled={saving || !newContent.trim()}
            className="w-full py-1 text-xs bg-accent text-white rounded hover:bg-accent-hover"
          >
            Save Note
          </button>
        </div>
      )}

      <div className="flex-1 overflow-y-auto p-1">
        {notes.map((note) => {
          const parsedCitations = parseCitations(note.citations);

          return (
            <div
              key={note.id}
              className="group px-2 py-2 rounded hover:bg-bg-tertiary border-b border-border/50"
            >
              <div className="flex items-start gap-1.5">
                <StickyNote className="w-3.5 h-3.5 text-text-muted mt-0.5 shrink-0" />
                <div className="flex-1 min-w-0">
                {editingNoteId === note.id ? (
                  <div className="space-y-1">
                    <input
                      type="text"
                      value={editTitle}
                      disabled={saving}
                      onChange={(e) => setEditTitle(e.target.value)}
                      className="w-full px-2 py-1 text-xs bg-bg-tertiary border border-border rounded text-text placeholder:text-text-muted focus:outline-none focus:border-accent"
                    />
                    <textarea
                      value={editContent}
                      disabled={saving}
                      onChange={(e) => setEditContent(e.target.value)}
                      rows={5}
                      className="w-full px-2 py-1 text-xs bg-bg-tertiary border border-border rounded text-text placeholder:text-text-muted focus:outline-none focus:border-accent resize-none"
                    />
                    <div className="flex items-center gap-1">
                      <button
                        onClick={handleSaveEdit}
                        disabled={saving}
                        className="inline-flex items-center gap-1 px-2 py-1 text-[10px] bg-accent text-white rounded hover:bg-accent-hover"
                      >
                        <Save className="w-3 h-3" />
                        Save
                      </button>
                      <button
                        onClick={cancelEditing}
                        disabled={saving}
                        className="inline-flex items-center gap-1 px-2 py-1 text-[10px] bg-bg-tertiary text-text-secondary rounded hover:text-text"
                      >
                        <X className="w-3 h-3" />
                        Cancel
                      </button>
                    </div>
                  </div>
                ) : (
                  <>
                    <p className="text-xs font-medium text-text truncate">
                      {note.title || "Untitled"}
                    </p>
                    <p className="text-[10px] text-text-muted line-clamp-2 mt-0.5">
                      {note.content.slice(0, 100)}
                    </p>
                    <div className="flex items-center gap-1 mt-1 text-[10px] text-text-muted">
                      <span>{note.note_type === "saved_response" ? "Saved" : "Manual"}</span>
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
                </div>
                <div className="flex items-center gap-0.5 opacity-0 group-hover:opacity-100">
                {editingNoteId !== note.id && (
                  <button
                    onClick={() => startEditing(note.id, note.title, note.content)}
                    className="p-0.5 rounded hover:bg-bg-tertiary"
                  >
                    <Pencil className="w-3 h-3 text-text-muted" />
                  </button>
                )}
                <button
                  onClick={() => void togglePin(notebookId, note.id).catch(() => undefined)}
                  className="p-0.5 rounded hover:bg-bg-tertiary"
                >
                  {note.pinned ? (
                    <PinOff className="w-3 h-3 text-accent" />
                  ) : (
                    <Pin className="w-3 h-3 text-text-muted" />
                  )}
                </button>
                <button
                  onClick={() => void deleteNote(notebookId, note.id).catch(() => undefined)}
                  aria-label={`Delete note ${note.title || 'Untitled'}`}
                  className="p-0.5 rounded hover:bg-error/20 text-text-muted hover:text-error"
                >
                  <Trash2 className="w-3 h-3" />
                </button>
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
