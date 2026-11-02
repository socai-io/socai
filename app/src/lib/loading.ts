import { esc } from './html';
import './loading.css';

export function renderResourceLoading(label: string, variant: 'conversation' | 'list' | 'content' = 'conversation'): string {
  return `<section class="resource-loading resource-loading--${variant}" role="status" aria-label="${esc(label)}" aria-busy="true" data-resource-loading>
    <span class="sr-only">${esc(label)}</span><div class="resource-loading__shape" aria-hidden="true"><i></i><i></i><i></i><i></i><i></i></div>
  </section>`;
}

export function renderResourceError(label: string, taskId: string, retryLabel: string): string {
  return `<section class="resource-load-error" role="status" data-resource-error><p>${esc(label)}</p><button type="button" class="btn-ghost btn-compact" data-task-detail-retry="${esc(taskId)}">${esc(retryLabel)}</button></section>`;
}
