/** The History API binding for the canonical route (docs/room-workbench.md,
 *  decision 2). Deliberately hand-rolled: the app's entire runtime dependency
 *  set is react + react-dom, and adding a router library is a decision this
 *  record explicitly declines to make for ~60 lines of pushState.
 */

import { useCallback, useEffect, useMemo, useState } from 'react';
import { parseRoute, routePath, searchWithoutLegacyTab } from './routes';
import type { Route } from './routes';

export type NavigateOptions = {
  /** Replace the current entry instead of pushing one. Use for corrections the
   *  user did not perform — canonicalizing `/` to `/rooms`, restoring the last
   *  room at boot — so Back never walks through a state they never saw. */
  replace?: boolean;
};

export type Navigate = (route: Route, options?: NavigateOptions) => void;

/** Build the full URL for a route, preserving the query and fragment.
 *
 *  Preserving `search` is a contract, not a nicety: `?daemon=` selects the
 *  daemon and `?mock…` installs the e2e fixtures. A redirect that dropped it
 *  would point the client at a different daemon and silently unfixture the
 *  suite — which passes, for the wrong reason. `?tab=` is the one key we
 *  consume, and only once it has been migrated onto a route. */
function urlFor(route: Route): string {
  const search = searchWithoutLegacyTab(window.location.search);
  return `${routePath(route)}${search}${window.location.hash}`;
}

/** Subscribe to the canonical route.
 *
 *  `popstate` covers Back/Forward; pushState/replaceState do not fire it, so
 *  navigate() updates the subscriber itself. */
export function useRoute(): [Route, Navigate] {
  const [pathname, setPathname] = useState(() => window.location.pathname);

  useEffect(() => {
    const onPopState = () => setPathname(window.location.pathname);
    window.addEventListener('popstate', onPopState);
    return () => window.removeEventListener('popstate', onPopState);
  }, []);

  const route = useMemo(() => parseRoute(pathname), [pathname]);

  const navigate = useCallback<Navigate>((next, options) => {
    const url = urlFor(next);
    const path = routePath(next);
    // Re-navigating to the route we are already on must not push a duplicate
    // entry — otherwise Back becomes a no-op the user has to press twice.
    // A replace still runs: it is how a legacy `?tab=` link gets rewritten.
    if (path === window.location.pathname && !options?.replace) return;
    if (options?.replace) window.history.replaceState(null, '', url);
    else window.history.pushState(null, '', url);
    setPathname(path);
  }, []);

  return [route, navigate];
}
