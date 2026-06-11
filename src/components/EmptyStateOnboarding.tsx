import { FileUp, Plus, Sparkles } from "lucide-react";

interface EmptyStateOnboardingProps {
  onCreateEmptyNotebook: () => void;
  onTrySampleNotebook: () => void;
  onImportFiles: () => void;
}

export function EmptyStateOnboarding({
  onCreateEmptyNotebook,
  onTrySampleNotebook,
  onImportFiles,
}: EmptyStateOnboardingProps) {
  return (
    <main className="min-h-full flex-1 overflow-auto p-6">
      <div className="mx-auto flex w-full max-w-4xl flex-col items-center gap-8 rounded-lg border border-border bg-bg-secondary p-8 text-center">
        <div>
          <h2 className="gloss-serif text-3xl text-text">Welcome to Gloss</h2>
          <p className="mt-3 text-sm text-text-secondary">
            Your local-first notebook for chat, sources, and notes
          </p>
        </div>

        <div className="grid w-full gap-3 sm:grid-cols-3">
          <button
            type="button"
            onClick={onCreateEmptyNotebook}
            className="rounded-lg border border-accent/35 bg-accent/12 px-5 py-4 text-sm font-medium text-accent transition hover:border-accent/65 hover:bg-accent/20"
          >
            <Plus className="mx-auto mb-2 h-5 w-5" />
            Create empty notebook
          </button>
          <button
            type="button"
            onClick={onTrySampleNotebook}
            className="rounded-lg border border-border bg-bg-tertiary px-5 py-4 text-sm font-medium text-text transition hover:border-accent/35 hover:text-accent"
          >
            <Sparkles className="mx-auto mb-2 h-5 w-5" />
            Try a sample notebook
          </button>
          <button
            type="button"
            onClick={onImportFiles}
            className="rounded-lg border border-border bg-bg-tertiary px-5 py-4 text-sm font-medium text-text transition hover:border-accent/35 hover:text-accent"
            title="Open and focus Sources panel to import files"
          >
            <FileUp className="mx-auto mb-2 h-5 w-5" />
            Import files
          </button>
        </div>

        <p className="text-sm text-text-muted">Or drop files anywhere to import</p>
      </div>
    </main>
  );
}
