import { useEffect, useState } from "react";
import { SourcesPanel } from "../sources/SourcesPanel";
import { ChatPanel } from "../chat/ChatPanel";
import { NotesPanel } from "../notes/NotesPanel";
import { useSourceStore } from "../../stores/sourceStore";
import { useChatStore } from "../../stores/chatStore";
import { useNoteStore } from "../../stores/noteStore";
import { PanelLeftClose, PanelLeftOpen, PanelRightClose, PanelRightOpen } from "lucide-react";

interface PanelLayoutProps {
  notebookId: string;
}

export function PanelLayout({ notebookId }: PanelLayoutProps) {
  const loadSources = useSourceStore((s) => s.loadSources);
  const loadStats = useSourceStore((s) => s.loadStats);
  const loadConversations = useChatStore((s) => s.loadConversations);
  const loadSuggestedQuestions = useChatStore((s) => s.loadSuggestedQuestions);
  const loadNotes = useNoteStore((s) => s.loadNotes);
  const [leftWidth, setLeftWidth] = useState(() => Number(localStorage.getItem("gloss:layout:leftWidth") || 288));
  const [rightWidth, setRightWidth] = useState(() => Number(localStorage.getItem("gloss:layout:rightWidth") || 288));
  const [leftCollapsed, setLeftCollapsed] = useState(() => localStorage.getItem("gloss:layout:leftCollapsed") === "1");
  const [rightCollapsed, setRightCollapsed] = useState(() => localStorage.getItem("gloss:layout:rightCollapsed") === "1");

  useEffect(() => {
    loadSources(notebookId);
    loadStats(notebookId);
    loadConversations(notebookId);
    loadSuggestedQuestions(notebookId);
    loadNotes(notebookId);
  }, [notebookId]);

  useEffect(() => {
    localStorage.setItem("gloss:layout:leftWidth", String(leftWidth));
  }, [leftWidth]);

  useEffect(() => {
    localStorage.setItem("gloss:layout:rightWidth", String(rightWidth));
  }, [rightWidth]);

  useEffect(() => {
    localStorage.setItem("gloss:layout:leftCollapsed", leftCollapsed ? "1" : "0");
  }, [leftCollapsed]);

  useEffect(() => {
    localStorage.setItem("gloss:layout:rightCollapsed", rightCollapsed ? "1" : "0");
  }, [rightCollapsed]);

  const startResize = (side: "left" | "right", startX: number) => {
    const initialWidth = side === "left" ? leftWidth : rightWidth;
    const onMove = (event: MouseEvent) => {
      const delta = side === "left" ? event.clientX - startX : startX - event.clientX;
      const next = Math.min(460, Math.max(220, initialWidth + delta));
      if (side === "left") setLeftWidth(next);
      else setRightWidth(next);
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  return (
    <div className="flex-1 flex overflow-hidden">
      {!leftCollapsed ? (
        <div className="relative border-r border-border overflow-y-auto" style={{ width: leftWidth }}>
          <button
            onClick={() => setLeftCollapsed(true)}
            className="absolute right-2 top-2 z-10 rounded bg-bg-secondary p-1 text-text-muted hover:bg-bg-tertiary hover:text-text"
            title="Collapse sources"
          >
            <PanelLeftClose className="h-3.5 w-3.5" />
          </button>
          <SourcesPanel notebookId={notebookId} />
          <div
            role="separator"
            className="absolute right-0 top-0 h-full w-1 cursor-col-resize hover:bg-accent/40"
            onMouseDown={(event) => startResize("left", event.clientX)}
          />
        </div>
      ) : (
        <button
          onClick={() => setLeftCollapsed(false)}
          className="w-8 border-r border-border bg-bg-secondary text-text-muted hover:bg-bg-tertiary hover:text-text flex items-center justify-center"
          title="Expand sources"
        >
          <PanelLeftOpen className="h-4 w-4" />
        </button>
      )}
      <div className="flex-1 flex flex-col overflow-hidden">
        <ChatPanel notebookId={notebookId} />
      </div>
      {!rightCollapsed ? (
        <div className="relative border-l border-border overflow-y-auto" style={{ width: rightWidth }}>
          <button
            onClick={() => setRightCollapsed(true)}
            className="absolute left-2 top-2 z-10 rounded bg-bg-secondary p-1 text-text-muted hover:bg-bg-tertiary hover:text-text"
            title="Collapse notes"
          >
            <PanelRightClose className="h-3.5 w-3.5" />
          </button>
          <NotesPanel notebookId={notebookId} />
          <div
            role="separator"
            className="absolute left-0 top-0 h-full w-1 cursor-col-resize hover:bg-accent/40"
            onMouseDown={(event) => startResize("right", event.clientX)}
          />
        </div>
      ) : (
        <button
          onClick={() => setRightCollapsed(false)}
          className="w-8 border-l border-border bg-bg-secondary text-text-muted hover:bg-bg-tertiary hover:text-text flex items-center justify-center"
          title="Expand notes"
        >
          <PanelRightOpen className="h-4 w-4" />
        </button>
      )}
    </div>
  );
}
