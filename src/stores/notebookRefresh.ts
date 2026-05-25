let refreshNotebookListHandler: (() => Promise<void>) | null = null;

export function registerNotebookListRefresher(handler: () => Promise<void>) {
  refreshNotebookListHandler = handler;
}

export async function refreshNotebookList() {
  if (refreshNotebookListHandler) {
    await refreshNotebookListHandler();
  }
}
