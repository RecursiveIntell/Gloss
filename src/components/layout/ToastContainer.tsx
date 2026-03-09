import { X, AlertCircle, CheckCircle2, Info, AlertTriangle } from "lucide-react";
import { useToastStore, type Toast } from "../../stores/toastStore";

const iconMap: Record<Toast["type"], React.ReactNode> = {
  error: <AlertCircle className="w-4 h-4 text-red-400 shrink-0" />,
  success: <CheckCircle2 className="w-4 h-4 text-green-400 shrink-0" />,
  info: <Info className="w-4 h-4 text-blue-400 shrink-0" />,
  warning: <AlertTriangle className="w-4 h-4 text-yellow-400 shrink-0" />,
};

const borderMap: Record<Toast["type"], string> = {
  error: "border-red-500/40",
  success: "border-green-500/40",
  info: "border-blue-500/40",
  warning: "border-yellow-500/40",
};

function ToastItem({ toast }: { toast: Toast }) {
  const { removeToast } = useToastStore();

  return (
    <div
      className={`flex items-start gap-2 px-3 py-2 rounded-lg border bg-bg-secondary
                  shadow-lg max-w-sm animate-in slide-in-from-right ${borderMap[toast.type]}`}
    >
      {iconMap[toast.type]}
      <div className="flex-1 min-w-0">
        <p className="text-xs font-medium text-text">{toast.title}</p>
        <p className="text-xs text-text-secondary truncate">{toast.message}</p>
      </div>
      <button
        onClick={() => removeToast(toast.id)}
        className="text-text-muted hover:text-text shrink-0"
      >
        <X className="w-3.5 h-3.5" />
      </button>
    </div>
  );
}

export function ToastContainer() {
  const { toasts } = useToastStore();

  if (toasts.length === 0) return null;

  return (
    <div className="fixed bottom-10 right-4 z-50 flex flex-col gap-2">
      {toasts.map((t) => (
        <ToastItem key={t.id} toast={t} />
      ))}
    </div>
  );
}
