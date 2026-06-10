import { useEffect, useRef, useState } from "react";
import type { KeyboardEvent, ReactNode } from "react";
import { SourcesPanel } from "../sources/SourcesPanel";
import { ChatPanel } from "../chat/ChatPanel";
import { NotesPanel } from "../notes/NotesPanel";
import { StudioPanel } from "../studio/StudioPanel";
import { EvidencePanel } from "../inspector/EvidencePanel";
import { PromptPanel } from "../inspector/PromptPanel";
import { ReceiptPanel } from "../inspector/ReceiptPanel";
import { DiagnosticsPanel } from "../inspector/DiagnosticsPanel";
import { useSourceStore } from "../../stores/sourceStore";
import { useChatStore } from "../../stores/chatStore";
import { useNoteStore } from "../../stores/noteStore";
import {
  Activity,
  FileText,
  MessageSquare,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen,
  ScrollText,
  ShieldCheck,
  Sparkles,
  StickyNote,
} from "lucide-react";

type InspectorTab = "notes" | "studio" | "prompt" | "evidence" | "receipt" | "diagnostics" | "sources";

const PANEL_MIN_WIDTH = 320;
const PANEL_MAX_WIDTH = 700;
const LEFT_PANEL_FALLBACK_WIDTH = 320;
const RIGHT_PANEL_FALLBACK_WIDTH = 560;

export function clampPanelWidth(value: unknown, fallback: number): number {
  const parsed = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(parsed)) return fallback;
  return Math.min(PANEL_MAX_WIDTH, Math.max(PANEL_MIN_WIDTH, parsed));
}

function readStoredPanelWidth(key: string, fallback: number): number {
  const next = clampPanelWidth(localStorage.getItem(key), fallback);
  localStorage.setItem(key, String(next));
  return next;
}

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
  const [leftWidth, setLeftWidth] = useState(() => readStoredPanelWidth("gloss:layout:leftWidth", LEFT_PANEL_FALLBACK_WIDTH));
  const [rightWidth, setRightWidth] = useState(() => readStoredPanelWidth("gloss:layout:rightWidth", RIGHT_PANEL_FALLBACK_WIDTH));
  const [leftCollapsed, setLeftCollapsed] = useState(() => localStorage.getItem("gloss:layout:leftCollapsed") !== "0");
  const [rightCollapsed, setRightCollapsed] = useState(() => localStorage.getItem("gloss:layout:rightCollapsed") !== "0");
  const [inspectorTab, setInspectorTab] = useState<InspectorTab>("notes");

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

  const dragActiveRef = useRef(false);
  const dragMoveHandlerRef = useRef<((event: MouseEvent) => void) | null>(null);
  const dragUpHandlerRef = useRef<(() => void) | null>(null);

  const startResize = (side: "left" | "right", startX: number) => {
    const initialWidth = side === "left" ? leftWidth : rightWidth;
    const onMove = (event: MouseEvent) => {
      const delta = side === "left" ? event.clientX - startX : startX - event.clientX;
      const next = clampPanelWidth(initialWidth + delta, initialWidth);
      if (side === "left") setLeftWidth(next);
      else setRightWidth(next);
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      dragActiveRef.current = false;
      dragMoveHandlerRef.current = null;
      dragUpHandlerRef.current = null;
    };
    dragActiveRef.current = true;
    dragMoveHandlerRef.current = onMove;
    dragUpHandlerRef.current = onUp;
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  const resizeWithKeyboard = (side: "left" | "right", event: KeyboardEvent<HTMLDivElement>) => {
    const currentWidth = side === "left" ? leftWidth : rightWidth;
    const setWidth = side === "left" ? setLeftWidth : setRightWidth;
    const direction = side === "left" ? 1 : -1;
    let next: number | null = null;
    if (event.key === "ArrowLeft") next = currentWidth - 24 * direction;
    if (event.key === "ArrowRight") next = currentWidth + 24 * direction;
    if (event.key === "Home") next = PANEL_MIN_WIDTH;
    if (event.key === "End") next = PANEL_MAX_WIDTH;
    if (next == null) return;
    event.preventDefault();
    setWidth(clampPanelWidth(next, currentWidth));
  };

  useEffect(() => {
    return () => {
      if (dragActiveRef.current) {
        if (dragMoveHandlerRef.current) window.removeEventListener("mousemove", dragMoveHandlerRef.current);
        if (dragUpHandlerRef.current) window.removeEventListener("mouseup", dragUpHandlerRef.current);
        dragActiveRef.current = false;
      }
    };
  }, []);

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
            tabIndex={0}
            aria-label="Resize sources panel"
            aria-orientation="vertical"
            aria-valuemin={PANEL_MIN_WIDTH}
            aria-valuemax={PANEL_MAX_WIDTH}
            aria-valuenow={leftWidth}
            className="absolute right-0 top-0 h-full w-1 cursor-col-resize hover:bg-accent/40 focus:bg-accent/40 focus:outline-none"
            onMouseDown={(event) => startResize("left", event.clientX)}
            onKeyDown={(event) => resizeWithKeyboard("left", event)}
          />
        </div>
      ) : null}
      <div className="flex min-w-0 flex-1 flex-col overflow-hidden">
        <ChatPanel notebookId={notebookId} />
      </div>
      {!rightCollapsed ? (
        <div className="gloss-panel relative flex shrink-0 flex-col overflow-hidden border-l border-border" style={{ width: rightWidth }}>
          <DrawerHeader
            title="Inspector Dock"
            subtitle={`${notes.length} saved notes`}
            onClose={() => setRightCollapsed(true)}
            closeSide="right"
          />
          <InspectorDock
            notebookId={notebookId}
            activeTab={inspectorTab}
            onTabChange={setInspectorTab}
          />
          <div
            role="separator"
            tabIndex={0}
            aria-label="Resize inspector dock"
            aria-orientation="vertical"
            aria-valuemin={PANEL_MIN_WIDTH}
            aria-valuemax={PANEL_MAX_WIDTH}
            aria-valuenow={rightWidth}
            className="absolute left-0 top-0 h-full w-1 cursor-col-resize hover:bg-accent/40 focus:bg-accent/40 focus:outline-none"
            onMouseDown={(event) => startResize("right", event.clientX)}
            onKeyDown={(event) => resizeWithKeyboard("right", event)}
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

function InspectorDock({
  notebookId,
  activeTab,
  onTabChange,
}: {
  notebookId: string;
  activeTab: InspectorTab;
  onTabChange: (tab: InspectorTab) => void;
}) {
  const tabs = [
    { id: "notes", label: "Notes", icon: <StickyNote className="h-3.5 w-3.5" /> },
    { id: "studio", label: "Studio", icon: <Sparkles className="h-3.5 w-3.5" /> },
    { id: "evidence", label: "Evidence", icon: <ShieldCheck className="h-3.5 w-3.5" /> },
    { id: "prompt", label: "Prompt", icon: <ScrollText className="h-3.5 w-3.5" /> },
    { id: "receipt", label: "Receipt", icon: <FileText className="h-3.5 w-3.5" /> },
    { id: "diagnostics", label: "Health", icon: <Activity className="h-3.5 w-3.5" /> },
    { id: "sources", label: "Sources", icon: <PanelLeftOpen className="h-3.5 w-3.5" /> },
  ] as const;

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
      <div className="flex border-b border-border bg-bg-secondary/70">
        {tabs.map((tab) => (
          <button
            key={tab.id}
            type="button"
            title={tab.label}
            aria-label={`Inspector tab: ${tab.label}`}
            onClick={() => onTabChange(tab.id)}
            className={`flex h-9 flex-1 items-center justify-center gap-1 text-[11px] ${
              activeTab === tab.id ? "bg-bg-tertiary text-text" : "text-text-muted hover:text-text"
            }`}
          >
            {tab.icon}
            <span>{tab.label}</span>
          </button>
        ))}
      </div>
      <div className="min-h-0 flex-1 overflow-hidden">
        {activeTab === "notes" ? <NotesPanel notebookId={notebookId} /> : null}
        {activeTab === "studio" ? <StudioPanel notebookId={notebookId} /> : null}
        {activeTab === "evidence" ? <EvidencePanel /> : null}
        {activeTab === "prompt" ? <PromptPanel /> : null}
        {activeTab === "receipt" ? <ReceiptPanel /> : null}
        {activeTab === "diagnostics" ? <DiagnosticsPanel notebookId={notebookId} /> : null}
        {activeTab === "sources" ? <SourcesPanel notebookId={notebookId} /> : null}
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