import { listen } from "@tauri-apps/api/event";
import type { ChatTokenPayload, SourceStatusPayload } from "./types";

export function onChatToken(
  callback: (payload: ChatTokenPayload) => void
): Promise<() => void> {
  return listen<ChatTokenPayload>("chat:token", (event) => {
    callback(event.payload);
  }).then((unlisten) => unlisten);
}

export function onSourceStatus(
  callback: (payload: SourceStatusPayload) => void
): Promise<() => void> {
  return listen<SourceStatusPayload>("source:status", (event) => {
    callback(event.payload);
  }).then((unlisten) => unlisten);
}
