import { useSettingsStore } from "../../stores/settingsStore";
import { Wifi, WifiOff } from "lucide-react";
import { useState, useEffect } from "react";

export function StatusBar() {
  const activeModel = useSettingsStore((s) => s.activeModel);
  const [connected, setConnected] = useState(false);
  const testProvider = useSettingsStore((s) => s.testProvider);

  useEffect(() => {
    testProvider("ollama").then(setConnected);
    const interval = setInterval(() => {
      testProvider("ollama").then(setConnected);
    }, 30000);
    return () => clearInterval(interval);
  }, []);

  return (
    <div className="h-7 bg-bg-secondary border-t border-border flex items-center px-3 text-xs text-text-muted gap-4">
      <div className="flex items-center gap-1.5">
        {connected ? (
          <Wifi className="w-3 h-3 text-success" />
        ) : (
          <WifiOff className="w-3 h-3 text-error" />
        )}
        <span>{connected ? "Connected" : "Disconnected"}</span>
      </div>
      <div className="flex items-center gap-1.5">
        <span>Model: {activeModel}</span>
      </div>
    </div>
  );
}
