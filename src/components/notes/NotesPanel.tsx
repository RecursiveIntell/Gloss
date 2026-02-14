import { useState } from "react";
import { useNoteStore } from "../../stores/noteStore";
import { StickyNote, Plus, Pin, PinOff, Trash2 } from "lucide-react";

interface NotesPanelProps {
  notebookId: string;
}

export function NotesPanel({ notebookId }: NotesPanelProps) {
  const { notes, createNote, togglePin, deleteNote } = useNoteStore();
  const [showCreate, setShowCreate] = useState(false);
  const [newTitle, setNewTitle] = useState("");
  const [newContent, setNewContent] = useState("");

  const handleCreate = async () => {
    if (!newContent.trim()) return;
    await createNote(notebookId, newTitle || "Untitled Note", newContent);
    setNewTitle("");
    setNewContent("");
    setShowCreate(false);
  };

  return (
    <div className="flex flex-col h-full">
      <div className="p-3 border-b border-border flex items-center justify-between">
        <h2 className="text-sm font-semibold text-text">Notes</h2>
        <button
          onClick={() => setShowCreate(!showCreate)}
          className="p-1 rounded hover:bg-bg-tertiary text-text-secondary hover:text-text"
        >
          <Plus className="w-4 h-4" />
        </button>
      </div>

      {showCreate && (
        <div className="p-2 border-b border-border space-y-1">
          <input
            type="text"
            value={newTitle}
            onChange={(e) => setNewTitle(e.target.value)}
            placeholder="Title..."
            className="w-full px-2 py-1 text-xs bg-bg-tertiary border border-border rounded text-text placeholder:text-text-muted focus:outline-none focus:border-accent"
          />
          <textarea
            value={newContent}
            onChange={(e) => setNewContent(e.target.value)}
            placeholder="Write your note..."
            rows={4}
            className="w-full px-2 py-1 text-xs bg-bg-tertiary border border-border rounded text-text placeholder:text-text-muted focus:outline-none focus:border-accent resize-none"
          />
          <button
            onClick={handleCreate}
            className="w-full py-1 text-xs bg-accent text-white rounded hover:bg-accent-hover"
          >
            Save Note
          </button>
        </div>
      )}

      <div className="flex-1 overflow-y-auto p-1">
        {notes.map((note) => (
          <div
            key={note.id}
            className="group px-2 py-2 rounded hover:bg-bg-tertiary border-b border-border/50"
          >
            <div className="flex items-start gap-1.5">
              <StickyNote className="w-3.5 h-3.5 text-text-muted mt-0.5 shrink-0" />
              <div className="flex-1 min-w-0">
                <p className="text-xs font-medium text-text truncate">
                  {note.title || "Untitled"}
                </p>
                <p className="text-[10px] text-text-muted line-clamp-2 mt-0.5">
                  {note.content.slice(0, 100)}
                </p>
                <div className="flex items-center gap-1 mt-1 text-[10px] text-text-muted">
                  <span>{note.note_type === "saved_response" ? "Saved" : "Manual"}</span>
                </div>
              </div>
              <div className="flex items-center gap-0.5 opacity-0 group-hover:opacity-100">
                <button
                  onClick={() => togglePin(notebookId, note.id)}
                  className="p-0.5 rounded hover:bg-bg-tertiary"
                >
                  {note.pinned ? (
                    <PinOff className="w-3 h-3 text-accent" />
                  ) : (
                    <Pin className="w-3 h-3 text-text-muted" />
                  )}
                </button>
                <button
                  onClick={() => deleteNote(notebookId, note.id)}
                  className="p-0.5 rounded hover:bg-error/20 text-text-muted hover:text-error"
                >
                  <Trash2 className="w-3 h-3" />
                </button>
              </div>
            </div>
          </div>
        ))}

        {notes.length === 0 && (
          <p className="text-xs text-text-muted text-center mt-4 px-2">
            No notes yet. Create one or save a chat response.
          </p>
        )}
      </div>
    </div>
  );
}
