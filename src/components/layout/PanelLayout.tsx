import { useEffect } from "react";
import { SourcesPanel } from "../sources/SourcesPanel";
import { ChatPanel } from "../chat/ChatPanel";
import { NotesPanel } from "../notes/NotesPanel";
import { useSourceStore } from "../../stores/sourceStore";
import { useChatStore } from "../../stores/chatStore";
import { useNoteStore } from "../../stores/noteStore";

interface PanelLayoutProps {
  notebookId: string;
}

export function PanelLayout({ notebookId }: PanelLayoutProps) {
  const loadSources = useSourceStore((s) => s.loadSources);
  const loadStats = useSourceStore((s) => s.loadStats);
  const loadConversations = useChatStore((s) => s.loadConversations);
  const loadNotes = useNoteStore((s) => s.loadNotes);

  useEffect(() => {
    loadSources(notebookId);
    loadStats(notebookId);
    loadConversations(notebookId);
    loadNotes(notebookId);
  }, [notebookId]);

  return (
    <div className="flex-1 flex overflow-hidden">
      <div className="w-72 border-r border-border overflow-y-auto">
        <SourcesPanel notebookId={notebookId} />
      </div>
      <div className="flex-1 flex flex-col overflow-hidden">
        <ChatPanel notebookId={notebookId} />
      </div>
      <div className="w-72 border-l border-border overflow-y-auto">
        <NotesPanel notebookId={notebookId} />
      </div>
    </div>
  );
}
