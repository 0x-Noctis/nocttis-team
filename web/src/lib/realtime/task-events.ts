import type { TaskEventResponse } from '$lib/api/client';

export function taskEventStream(
  taskId: string,
  onEvent: (event: TaskEventResponse) => void,
  onError: () => void,
  initialIds: number[] = []
): () => void {
  const seen = new Set(initialIds);
  const source = new EventSource(`/api/v1/tasks/${encodeURIComponent(taskId)}/events/stream`);
  source.addEventListener('task_event', (message) => {
    const event = JSON.parse((message as MessageEvent<string>).data) as TaskEventResponse;
    if (seen.has(event.id)) return;
    seen.add(event.id);
    onEvent(event);
  });
  source.onerror = onError;
  return () => source.close();
}
