import { useCallback, useEffect, useState } from "react";
import type { AccountsState, Services } from "../services/types";

/** Liste des comptes et compte actif, tenue à jour par l'événement `auth://accounts`. null tant que non chargé. */
export function useAccounts(services: Services): { state: AccountsState | null; refresh: () => Promise<void> } {
  const [state, setState] = useState<AccountsState | null>(null);

  const refresh = useCallback(async () => {
    try { setState(await services.authAccounts()); } catch { /* conserve l'état précédent */ }
  }, [services]);

  useEffect(() => {
    let on = true;
    services.authAccounts().then((s) => { if (on) setState(s); }, () => {});
    const off = services.onAccountsState((s) => { if (on) setState(s); });
    return () => { on = false; off(); };
  }, [services]);

  return { state, refresh };
}
