import { useCallback, useMemo, useState } from "react";
import type { StudioOutput } from "../../lib/types";

interface QuizItem {
  question: string;
  options: string[];
  correct_index: number;
  explanation?: string;
  citation?: { source_title?: string; section?: string; [key: string]: unknown };
}

type AnswerState = "unanswered" | "correct" | "incorrect";

export function parseQuiz(raw: string | undefined): QuizItem[] {
  if (!raw) return [];
  try {
    const parsed = JSON.parse(raw) as Record<string, unknown> | unknown[];
    // Accepted shapes: bare array, {content: [...]}, StudioArtifact
    // {content: {questions: [...]}}, or {questions: [...]}. Items may use
    // options/correct_index (legacy) or choices/answer_index (backend).
    const content = !Array.isArray(parsed) ? parsed?.content : undefined;
    const items = Array.isArray(parsed)
      ? parsed
      : Array.isArray(content)
        ? content
        : Array.isArray((content as Record<string, unknown> | undefined)?.questions)
          ? ((content as Record<string, unknown>).questions as unknown[])
          : Array.isArray((parsed as Record<string, unknown>)?.questions)
            ? ((parsed as Record<string, unknown>).questions as unknown[])
            : [];
    return items
      .filter((item): item is Record<string, unknown> => item != null && typeof item === "object")
      .map((item) => {
        const options = Array.isArray(item.options)
          ? (item.options as unknown[])
          : Array.isArray(item.choices)
            ? (item.choices as unknown[])
            : [];
        const rawIndex =
          typeof item.correct_index === "number"
            ? item.correct_index
            : typeof item.answer_index === "number"
              ? item.answer_index
              : -1;
        const citations = Array.isArray(item.citations) ? (item.citations as unknown[]) : [];
        const citation =
          (item.citation as QuizItem["citation"]) ?? (citations[0] as QuizItem["citation"]);
        return {
          question: typeof item.question === "string" ? item.question : "",
          options: options.filter((option): option is string => typeof option === "string"),
          correct_index: rawIndex,
          explanation: typeof item.explanation === "string" ? item.explanation : undefined,
          citation,
        };
      })
      .filter(
        (item) =>
          item.question.length > 0 &&
          item.options.length >= 2 &&
          item.correct_index >= 0 &&
          item.correct_index < item.options.length
      );
  } catch {
    return [];
  }
}

function optionLabel(index: number): string {
  return String.fromCharCode(65 + index);
}

export function QuizWidget({ output }: { output: StudioOutput }) {
  const questions = useMemo(() => parseQuiz(output.raw_content), [output.raw_content]);
  const [currentIndex, setCurrentIndex] = useState(0);
  const [answerState, setAnswerState] = useState<AnswerState>("unanswered");
  const [selectedIndex, setSelectedIndex] = useState<number | null>(null);
  const [score, setScore] = useState(0);
  const [answered, setAnswered] = useState(0);
  const [done, setDone] = useState(false);

  const handleAnswer = useCallback(
    (optionIndex: number) => {
      if (answerState !== "unanswered") return;
      const q = questions[currentIndex];
      const isCorrect = optionIndex === q.correct_index;
      setSelectedIndex(optionIndex);
      setAnswerState(isCorrect ? "correct" : "incorrect");
      if (isCorrect) setScore((prev) => prev + 1);
      setAnswered((prev) => prev + 1);
    },
    [answerState, currentIndex, questions]
  );

  const handleNext = useCallback(() => {
    if (currentIndex + 1 < questions.length) {
      setCurrentIndex((prev) => prev + 1);
      setAnswerState("unanswered");
      setSelectedIndex(null);
    } else {
      setDone(true);
    }
  }, [currentIndex, questions.length]);

  const handleRetry = useCallback(() => {
    setCurrentIndex(0);
    setAnswerState("unanswered");
    setSelectedIndex(null);
    setScore(0);
    setAnswered(0);
    setDone(false);
  }, []);

  if (questions.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center gap-2 p-8">
        <p className="text-sm text-text-muted">No quiz data available.</p>
        <p className="text-xs text-text-muted">Generate a quiz Studio output first.</p>
      </div>
    );
  }

  if (done) {
    const pct = questions.length > 0 ? Math.round((score / questions.length) * 100) : 0;
    return (
      <div className="flex flex-col items-center gap-4 p-6">
        <div className="text-center">
          <div className="text-3xl font-semibold text-text">
            {score}/{questions.length}
          </div>
          <div className="mt-1 text-sm text-text-secondary">{pct}% correct</div>
          <div className="mt-1 text-xs text-text-muted">
            {pct >= 80 ? "Great job!" : pct >= 50 ? "Keep studying!" : "Keep at it!"}
          </div>
        </div>
        <button
          type="button"
          onClick={handleRetry}
          className="rounded border border-accent bg-accent/10 px-4 py-1.5 text-xs text-text hover:bg-accent/20"
        >
          Retry Quiz
        </button>
      </div>
    );
  }

  const q = questions[currentIndex];

  return (
    <div className="flex flex-col gap-4 p-4">
      {/* Score counter */}
      <div className="flex items-center justify-between text-xs text-text-muted">
        <span>
          Question {currentIndex + 1} of {questions.length}
        </span>
        <span className="font-medium text-text">
          Score: {score}/{answered}
        </span>
      </div>

      {/* Progress bar */}
      <div className="h-1 overflow-hidden rounded-full bg-bg-tertiary">
        <div
          className="h-full rounded-full bg-accent transition-all duration-300"
          style={{ width: `${(answered / questions.length) * 100}%` }}
        />
      </div>

      {/* Question */}
      <div className="rounded-lg border border-border bg-bg-secondary p-4">
        <p className="text-sm font-medium leading-relaxed text-text">{q.question}</p>
      </div>

      {/* Options */}
      <div className="flex flex-col gap-2">
        {q.options.map((option, i) => {
          const isAnswered = answerState !== "unanswered";
          const isCorrectOption = i === q.correct_index;
          const isSelectedWrong = isAnswered && i === selectedIndex && !isCorrectOption;

          return (
            <button
              key={option ?? `q-${i}`}
              type="button"
              disabled={isAnswered}
              onClick={() => handleAnswer(i)}
              className={`rounded border px-4 py-2.5 text-left text-xs transition-colors ${
                isSelectedWrong
                  ? "border-red-500/40 bg-red-500/10 text-red-400"
                  : isAnswered && isCorrectOption
                    ? "border-green-500/40 bg-green-500/10 text-green-400"
                    : isAnswered
                      ? "border-border bg-bg-tertiary text-text-muted opacity-60"
                      : "border-border bg-bg-tertiary text-text-secondary hover:border-accent/40 hover:bg-bg-secondary"
              }`}
            >
              <span className="gloss-mono mr-2 text-text-muted">{optionLabel(i)}.</span>
              {option}
            </button>
          );
        })}
      </div>

      {/* Explanation */}
      {answerState !== "unanswered" && (q.explanation || q.citation?.source_title) && (
        <div className="rounded-lg border border-border bg-bg-secondary p-3">
          <p className="text-xs font-medium text-text">Explanation</p>
          {q.explanation && (
            <p className="mt-1 text-xs leading-relaxed text-text-secondary">{q.explanation}</p>
          )}
          {q.citation?.source_title && (
            <p className="mt-2 text-[10px] text-text-muted">
              Source: {q.citation.source_title}
              {q.citation.section ? ` — ${q.citation.section}` : ""}
            </p>
          )}
        </div>
      )}

      {/* Next button */}
      {answerState !== "unanswered" && (
        <button
          type="button"
          onClick={handleNext}
          className="self-end rounded border border-accent bg-accent/10 px-4 py-1.5 text-xs text-text hover:bg-accent/20"
        >
          {currentIndex + 1 < questions.length ? "Next" : "See Results"}
        </button>
      )}
    </div>
  );
}
