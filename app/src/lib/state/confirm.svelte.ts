// A single confirmation question at a time ("Remove {name}?"), answered with a promise.

export interface ConfirmRequest {
  title: string;
  text: string;
  confirm: string;
  danger?: boolean;
  icon?: string;
}

export const confirmState = $state({
  request: null as (ConfirmRequest & { resolve: (ok: boolean) => void }) | null,
});

export function confirm(req: ConfirmRequest): Promise<boolean> {
  confirmState.request?.resolve(false);
  return new Promise((resolve) => {
    confirmState.request = { ...req, resolve };
  });
}

export function answer(ok: boolean): void {
  const r = confirmState.request;
  confirmState.request = null;
  r?.resolve(ok);
}
