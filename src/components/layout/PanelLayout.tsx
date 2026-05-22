import { useEffect, useState } from "react";
import type { ReactNode } from "react";
import { SourcesPanel } from "../sources/SourcesPanel";
import { ChatPanel } from "../chat/ChatPanel";
import { NotesPanel } from "../notes/NotesPanel";
import { useSourceStore } from "../../stores/sourceStore";
import { useChatStore } from "../../stores/chatStore";
import { useNoteStore } from "../../stores/noteStore";
import {
  FileText,
  MessageSquare,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen,
  StickyNote,
} from "lucide-react";

interface PanelLayoutProps {
  notebookId: string;
}

export function PanelLayout({ notebookId }: PanelLayoutProps) {
  const loadSources = useSourceStore((s) => s.loadSources);
  const loadStats = useSourceStore((s) => s.loadStats);
  const stats = useSourceStore((s) => s.stats);
  const selectedSourceIds = useSourceStore((s) => s.selectedSourceIds);
  const loadConversations = useChatStore((s) => s.loadConversations);
  const loadSuggestedQuestions = useChatStore((s) => s.loadSuggestedQuestions);
  const loadNotes = useNoteStore((s) => s.loadNotes);
  const notes = useNoteStore((s) => s.notes);
  const [leftWidth, setLeftWidth] = useState(() => Number(localStorage.getItem("gloss:layout:leftWidth") || 320));
  const [rightWidth, setRightWidth] = useState(() => Number(localStorage.getItem("gloss:layout:rightWidth") || 320));
  const [leftCollapsed, setLeftCollapsed] = useState(() => localStorage.getItem("gloss:layout:leftCollapsed") !== "0");
  const [rightCollapsed, setRightCollapsed] = useState(() => localStorage.getItem("gloss:layout:rightCollapsed") !== "0");

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
    <div className="flex flex-1 overflow-hidden">
      <div className="gloss-rail flex shrink-0 flex-col items-center gap-1 border-r py-2">
        <RailAction
          label={leftCollapsed ? "Open sources" : "Close sources"}
          active={!leftCollapsed}
          count={stats?.source_count ?? 0}
          onClick={() => setLeftCollapsed((collapsed) => !collapsed)}
        >
          {leftCollapsed ? <PanelLeftOpen className="h-4 w-4" /> : <FileText className="h-4 w-4" />}
        </RailAction>
        <RailAction label="Chat canvas" active={false}>
          <MessageSquare className="h-4 w-4" />
        </RailAction>
        <div className="flex-1" />
        {selectedSourceIds.size > 0 && (
          <span className="gloss-pill px-2 py-1 text-[10px]" title="Scoped sources">
            {selectedSourceIds.size}
          </span>
        )}
      </div>
      {!leftCollapsed ? (
        <div className="gloss-panel relative flex shrink-0 flex-col overflow-hidden border-r border-border" style={{ width: leftWidth }}>
          <DrawerHeader
            title="Sources"
            subtitle={`${stats?.source_count ?? 0} sources · ${stats?.chunk_count ?? 0} chunks`}
            onClose={() => setLeftCollapsed(true)}
            closeSide="left"
          />
          <SourcesPanel notebookId={notebookId} />
          <div
            role="separator"
            className="absolute right-0 top-0 h-full w-1 cursor-col-resize hover:bg-accent/40"
            onMouseDown={(event) => startResize("left", event.clientX)}
          />
        </div>
      ) : null}
      <div className="flex min-w-0 flex-1 flex-col overflow-hidden">
        <ChatPanel notebookId={notebookId} />
      </div>
      {!rightCollapsed ? (
        <div className="gloss-panel relative flex shrink-0 flex-col overflow-hidden border-l border-border" style={{ width: rightWidth }}>
          <DrawerHeader
            title="Notes"
            subtitle={`${notes.length} saved notes`}
            onClose={() => setRightCollapsed(true)}
            closeSide="right"
          />
          <NotesPanel notebookId={notebookId} />
          <div
            role="separator"
            className="absolute left-0 top-0 h-full w-1 cursor-col-resize hover:bg-accent/40"
            onMouseDown={(event) => startResize("right", event.clientX)}
          />
        </div>
      ) : null}
      <div className="gloss-rail flex shrink-0 flex-col items-center gap-1 border-l py-2">
        <RailAction
          label={rightCollapsed ? "Open notes" : "Close notes"}
          active={!rightCollapsed}
          count={notes.length}
          onClick={() => setRightCollapsed((collapsed) => !collapsed)}
        >
          {rightCollapsed ? <PanelRightOpen className="h-4 w-4" /> : <StickyNote className="h-4 w-4" />}
        </RailAction>
      </div>
    </div>
  );
}

function RailAction({
  label,
  active,
  count,
  children,
  onClick,
}: {
  label: string;
  active: boolean;
  count?: number;
  children: ReactNode;
  onClick?: () => void;
}) {
  return (
    <button
      type="button"
      title={label}
      onClick={onClick}
      className={`gloss-rail-button ${active ? "gloss-rail-button-active" : ""}`}
    >
      {children}
      {count !== undefined && count > 0 && <span className="gloss-badge">{count > 99 ? "99+" : count}</span>}
    </button>
  );
}

function DrawerHeader({
  title,
  subtitle,
  closeSide,
  onClose,
}: {
  title: string;
  subtitle: string;
  closeSide: "left" | "right";
  onClose: () => void;
}) {
  return (
    <div className="gloss-panel-header flex shrink-0 items-center gap-2 px-3 py-2">
      <div className="min-w-0 flex-1">
        <div className="gloss-serif truncate text-[17px] text-text">{title}</div>
        <div className="gloss-mono truncate text-[10px] uppercase tracking-[0.03em] text-text-muted">
          {subtitle}
        </div>
      </div>
      <button
        type="button"
        onClick={onClose}
        className="rounded border border-border p-1 text-text-muted hover:bg-bg-tertiary hover:text-text"
        title={`Close ${title.toLowerCase()}`}
      >
        {closeSide === "left" ? (
          <PanelLeftClose className="h-3.5 w-3.5" />
        ) : (
          <PanelRightClose className="h-3.5 w-3.5" />
        )}
      </button>
    </div>
  );
}
