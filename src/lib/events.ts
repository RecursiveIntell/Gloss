import { listen } from "@tauri-apps/api/event";
import type {
  ChatTokenPayload,
  ChatErrorPayload,
  SourceStatusPayload,
  EmbeddingModelPayload,
  JobCompletedPayload,
} from "./types";

export function onChatToken(
  callback: (payload: ChatTokenPayload) => void
): Promise<() => void> {
  return listen<ChatTokenPayload>("chat:token", (event) => {
    callback(event.payload);
  }).then((unlisten) => unlisten);
}

export function onChatError(
  callback: (payload: ChatErrorPayload) => void
): Promise<() => void> {
  return listen<ChatErrorPayload>("chat:error", (event) => {
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

export function onEmbeddingModelStatus(
  callback: (payload: EmbeddingModelPayload) => void
): Promise<() => void> {
  return listen<EmbeddingModelPayload>(
    "status:embedding_model",
    (event) => {
      callback(event.payload);
    }
  ).then((unlisten) => unlisten);
}

export function onJobCompleted(
  callback: (payload: JobCompletedPayload) => void
): Promise<() => void> {
  return listen<JobCompletedPayload>(
    "queue:job_completed",
    (event) => {
      callback(event.payload);
    }
  ).then((unlisten) => unlisten);
}
