import { isTaskApiError } from './i18n';

export interface ReplyModelChoice { provider: string; model: string }

/** Offer recovery, never change defaults or replay a message automatically. */
export function hostedReplyRecovery(
  task: {status: string; provider?: string | null; error?: string | null; final_text?: string | null},
  models: Array<{provider: string; has_key: boolean; recommended?: boolean; model_id?: string; default_model?: string}>,
  signedIn: boolean,
): ReplyModelChoice | null {
  if (!signedIn || !task.provider || task.provider === 'socai'
    || !['failed', 'interrupted'].includes(task.status)
    || ![task.error, task.final_text].some(detail => detail && (
      isTaskApiError(detail) || /credit_balance_exhausted/i.test(detail)
    ))) return null;
  const available = models.filter(m => m.provider === 'socai' && m.has_key && (m.model_id || m.default_model));
  const picked = available.find(m => m.recommended) ?? available[0];
  return picked ? {provider: 'socai', model: picked.model_id || picked.default_model!} : null;
}
