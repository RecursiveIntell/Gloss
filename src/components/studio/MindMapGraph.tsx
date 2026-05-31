import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  forceSimulation,
  forceLink,
  forceManyBody,
  forceCenter,
  forceCollide,
  type SimulationNodeDatum,
  type SimulationLinkDatum,
} from "d3-force";
import type { StudioOutput } from "../../lib/types";

interface MindMapNode {
  id: string;
  label: string;
  summary?: string;
  source_ids?: string[];
}

interface MindMapEdge {
  from: string;
  to: string;
  label?: string;
}

interface MindMapData {
  nodes: MindMapNode[];
  edges: MindMapEdge[];
}

interface GraphNode extends SimulationNodeDatum {
  id: string;
  label: string;
  summary?: string;
}

interface GraphLink extends SimulationLinkDatum<GraphNode> {
  label?: string;
}

function parseMindMap(raw: string | undefined): MindMapData | null {
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw);
    if (parsed && Array.isArray(parsed.nodes) && Array.isArray(parsed.edges)) {
      return parsed as MindMapData;
    }
    if (parsed && typeof parsed === "object" && parsed.content) {
      const c = parsed.content;
      if (c && Array.isArray(c.nodes) && Array.isArray(c.edges)) {
        return c as MindMapData;
      }
    }
    return null;
  } catch {
    return null;
  }
}

const NODE_W = 140;
const NODE_H = 48;
const NODE_RX = 8;

export function MindMapGraph({ output }: { output: StudioOutput }) {
  const data = useMemo(() => parseMindMap(output.raw_content), [output.raw_content]);
  const svgRef = useRef<SVGSVGElement>(null);
  const [hoveredId, setHoveredId] = useState<string | null>(null);
  const [graphNodes, setGraphNodes] = useState<GraphNode[]>([]);
  const [graphLinks, setGraphLinks] = useState<GraphLink[]>([]);
  const [viewBox, setViewBox] = useState("0 0 600 400");
  const [simRunning, setSimRunning] = useState(true);

  // Build graph and run simulation
  useEffect(() => {
    if (!data || data.nodes.length === 0) return;

    const nodeMap = new Map<string, GraphNode>();
    const nodes: GraphNode[] = data.nodes.map((n) => {
      const gn: GraphNode = { id: n.id, label: n.label, summary: n.summary };
      nodeMap.set(n.id, gn);
      return gn;
    });

    // Build links using node references
    const links: GraphLink[] = data.edges
      .filter((e) => nodeMap.has(e.from) && nodeMap.has(e.to))
      .map((e) => ({
        source: e.from,
        target: e.to,
        label: e.label,
      }));

    const sim = forceSimulation<GraphNode>(nodes)
      .force(
        "link",
        forceLink<GraphNode, GraphLink>(links)
          .id((d) => d.id)
          .distance(140)
      )
      .force("charge", forceManyBody().strength(-300))
      .force("center", forceCenter(300, 200))
      .force("collide", forceCollide(80))
      .stop();

    // Run simulation synchronously for initial layout
    const maxTicks = 200;
    for (let i = 0; i < maxTicks; i++) {
      sim.tick();
      if (sim.alpha() < 0.005) break;
    }
    sim.stop();

    setGraphNodes(nodes);
    setGraphLinks(links);
    setSimRunning(false);

    // Auto-fit viewBox
    if (nodes.length > 0) {
      const xs = nodes.map((n) => n.x ?? 0);
      const ys = nodes.map((n) => n.y ?? 0);
      const minX = Math.min(...xs) - NODE_W;
      const maxX = Math.max(...xs) + NODE_W;
      const minY = Math.min(...ys) - NODE_H;
      const maxY = Math.max(...ys) + NODE_H;
      const w = Math.max(400, maxX - minX + 40);
      const h = Math.max(300, maxY - minY + 40);
      setViewBox(`${minX - 20} ${minY - 20} ${w} ${h}`);
    }
  }, [data]);

  const handleNodeHover = useCallback((id: string | null) => {
    setHoveredId(id);
  }, []);

  if (!data || data.nodes.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center gap-2 p-8">
        <p className="text-sm text-text-muted">No mind map data available.</p>
        <p className="text-xs text-text-muted">Generate a mind map Studio output first.</p>
      </div>
    );
  }

  const hoveredNode = hoveredId ? graphNodes.find((n) => n.id === hoveredId) : null;

  return (
    <div className="relative flex h-full min-h-0 flex-col">
      <svg ref={svgRef} viewBox={viewBox} className="h-full w-full" preserveAspectRatio="xMidYMid meet">
        {/* Edges */}
        {graphLinks.map((link, i) => {
          const source = link.source as GraphNode;
          const target = link.target as GraphNode;
          const sx = source.x ?? 0;
          const sy = source.y ?? 0;
          const tx = target.x ?? 0;
          const ty = target.y ?? 0;
          const midX = (sx + tx) / 2;
          const midY = (sy + ty) / 2;

          return (
            <g key={`edge-${i}`}>
              <path
                d={`M${sx},${sy} Q${midX},${midY - 20} ${tx},${ty}`}
                fill="none"
                stroke="currentColor"
                className="text-border"
                strokeWidth={1.5}
              />
              {link.label && (
                <text
                  x={midX}
                  y={midY - 24}
                  textAnchor="middle"
                  className="fill-text-muted text-[9px]"
                >
                  {link.label}
                </text>
              )}
            </g>
          );
        })}

        {/* Nodes */}
        {graphNodes.map((node) => {
          const x = node.x ?? 0;
          const y = node.y ?? 0;
          const isHovered = hoveredId === node.id;

          return (
            <g
              key={node.id}
              className="cursor-pointer"
              onMouseEnter={() => handleNodeHover(node.id)}
              onMouseLeave={() => handleNodeHover(null)}
            >
              <rect
                x={x - NODE_W / 2}
                y={y - NODE_H / 2}
                width={NODE_W}
                height={NODE_H}
                rx={NODE_RX}
                className={`stroke-1 ${isHovered ? "stroke-accent" : "stroke-border"}`}
                fill="var(--color-bg-secondary, #1e1e2e)"
              />
              <text
                x={x}
                y={y + 1}
                textAnchor="middle"
                dominantBaseline="central"
                className="fill-text text-[11px] pointer-events-none"
              >
                {node.label.length > 22 ? node.label.slice(0, 22) + "…" : node.label}
              </text>
            </g>
          );
        })}
      </svg>

      {/* Tooltip */}
      {hoveredNode?.summary && (
        <div className="absolute bottom-3 left-3 right-3 rounded border border-border bg-bg-secondary px-3 py-2 shadow-lg">
          <p className="text-xs font-medium text-text">{hoveredNode.label}</p>
          <p className="mt-1 text-[11px] leading-relaxed text-text-secondary">{hoveredNode.summary}</p>
        </div>
      )}

      {simRunning && (
        <div className="absolute inset-0 flex items-center justify-center">
          <span className="text-xs text-text-muted">Rendering…</span>
        </div>
      )}
    </div>
  );
}
