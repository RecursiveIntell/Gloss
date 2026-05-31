import { useCallback, useMemo, useState } from "react";
import type { StudioOutput } from "../../lib/types";

interface FlashcardItem {
  front: string;
  back: string;
  citation?: { source_title?: string; section?: string; [key: string]: unknown };
  difficulty?: string;
}

type CardState = "known" | "review";

function parseCards(raw: string | undefined): FlashcardItem[] {
  if (!raw) return [];
  try {
    const parsed = JSON.parse(raw);
    if (Array.isArray(parsed)) return parsed as FlashcardItem[];
    if (parsed && typeof parsed === "object" && Array.isArray(parsed.content)) {
      return parsed.content as FlashcardItem[];
    }
    return [];
  } catch {
    return [];
  }
}

function difficultyColor(difficulty?: string): string {
  switch (difficulty) {
    case "easy":
      return "border-green-500/40 bg-green-500/10 text-green-400";
    case "medium":
      return "border-yellow-500/40 bg-yellow-500/10 text-yellow-400";
    case "hard":
      return "border-red-500/40 bg-red-500/10 text-red-400";
    default:
      return "border-border bg-bg-tertiary text-text-muted";
  }
}

export function FlashcardWidget({ output }: { output: StudioOutput }) {
  const cards = useMemo(() => parseCards(output.raw_content), [output.raw_content]);
  const [currentIndex, setCurrentIndex] = useState(0);
  const [flipped, setFlipped] = useState(false);
  const [ratings, setRatings] = useState<CardState[]>(() => new Array(cards.length).fill(null));
  const [done, setDone] = useState(false);

  const knownCount = ratings.filter((r) => r === "known").length;
  const reviewCount = ratings.filter((r) => r === "review").length;
  const remaining = cards.length - knownCount - reviewCount;

  const handleFlip = useCallback(() => {
    setFlipped((prev) => !prev);
  }, []);

  const handleRate = useCallback(
    (rating: CardState) => {
      const next = [...ratings];
      next[currentIndex] = rating;
      setRatings(next);
      setFlipped(false);

      if (currentIndex + 1 < cards.length) {
        setCurrentIndex((prev) => prev + 1);
      } else {
        setDone(true);
      }
    },
    [cards.length, currentIndex, ratings]
  );

  const handleReset = useCallback(() => {
    setCurrentIndex(0);
    setFlipped(false);
    setRatings(new Array(cards.length).fill(null));
    setDone(false);
  }, [cards.length]);

  if (cards.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center gap-2 p-8">
        <p className="text-sm text-text-muted">No flashcard data available.</p>
        <p className="text-xs text-text-muted">Generate a flashcards Studio output first.</p>
      </div>
    );
  }

  if (done) {
    const needReview = cards.filter((_, i) => ratings[i] === "review");
    return (
      <div className="flex flex-col items-center gap-4 p-6">
        <div className="text-center">
          <div className="text-2xl font-semibold text-text">
            {knownCount} known, {reviewCount} to review
          </div>
          <div className="mt-1 text-xs text-text-muted">
            {knownCount + reviewCount} of {cards.length} cards completed
          </div>
        </div>
        {needReview.length > 0 && (
          <div className="w-full max-w-md space-y-2">
            <p className="text-xs font-medium text-text-muted">Cards to review:</p>
            {needReview.map((card, i) => (
              <div
                key={i}
                className="rounded border border-border bg-bg-secondary px-3 py-2 text-xs text-text-secondary"
              >
                <span className="font-medium text-text">{card.front}</span>
                {card.citation?.source_title && (
                  <span className="ml-2 text-text-muted">— {card.citation.source_title}</span>
                )}
              </div>
            ))}
          </div>
        )}
        <button
          type="button"
          onClick={handleReset}
          className="rounded border border-accent bg-accent/10 px-4 py-1.5 text-xs text-text hover:bg-accent/20"
        >
          Review again
        </button>
      </div>
    );
  }

  const card = cards[currentIndex];

  return (
    <div className="flex flex-col items-center gap-4 p-4">
      {/* Progress bar */}
      <div className="w-full max-w-md">
        <div className="mb-1 flex items-center justify-between text-[11px] text-text-muted">
          <span>
            Card {currentIndex + 1}/{cards.length}
          </span>
          <span>
            {knownCount} known &middot; {reviewCount} review &middot; {remaining} remaining
          </span>
        </div>
        <div className="h-1 overflow-hidden rounded-full bg-bg-tertiary">
          <div
            className="h-full rounded-full bg-accent transition-all duration-300"
            style={{ width: `${((knownCount + reviewCount) / cards.length) * 100}%` }}
          />
        </div>
      </div>

      {/* Card */}
      <div
        className="w-full max-w-md cursor-pointer perspective-[800px]"
        onClick={handleFlip}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") handleFlip();
        }}
        role="button"
        tabIndex={0}
      >
        <div
          className={`relative min-h-[180px] rounded-lg border border-border bg-bg-secondary transition-transform duration-500 [transform-style:preserve-3d] ${
            flipped ? "[transform:rotateY(180deg)]" : ""
          }`}
        >
          {/* Front */}
          <div className="absolute inset-0 flex flex-col [backface-visibility:hidden]">
            <div className="flex flex-1 items-center justify-center p-6 text-center">
              <p className="text-sm leading-relaxed text-text">{card.front}</p>
            </div>
            {card.difficulty && (
              <div className="absolute right-2 top-2">
                <span
                  className={`inline-block rounded border px-1.5 py-0.5 text-[10px] leading-none ${difficultyColor(card.difficulty)}`}
                >
                  {card.difficulty}
                </span>
              </div>
            )}
            <div className="border-t border-border px-3 py-1.5 text-[10px] text-text-muted">
              Click to flip
            </div>
          </div>

          {/* Back */}
          <div className="absolute inset-0 flex flex-col [backface-visibility:hidden] [transform:rotateY(180deg)]">
            <div className="flex flex-1 items-center justify-center p-6 text-center">
              <p className="text-sm leading-relaxed text-text">{card.back}</p>
            </div>
            {card.citation?.source_title && (
              <div className="border-t border-border px-3 py-1.5 text-[10px] text-text-muted">
                {card.citation.source_title}
                {card.citation.section ? ` — ${card.citation.section}` : ""}
              </div>
            )}
          </div>
        </div>
      </div>

      {/* Rating buttons (only visible after flip) */}
      <div
        className={`flex gap-3 transition-all duration-300 ${
          flipped ? "translate-y-0 opacity-100" : "pointer-events-none translate-y-2 opacity-0"
        }`}
      >
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            handleRate("known");
          }}
          className="rounded border border-green-500/40 bg-green-500/10 px-4 py-1.5 text-xs text-green-400 hover:bg-green-500/20"
        >
          Know
        </button>
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            handleRate("review");
          }}
          className="rounded border border-yellow-500/40 bg-yellow-500/10 px-4 py-1.5 text-xs text-yellow-400 hover:bg-yellow-500/20"
        >
          Review
        </button>
      </div>
    </div>
  );
}
