/**
 * Poll while the tab is in front, and catch up the moment it comes back.
 *
 * `setInterval` on its own keeps asking a server nobody is looking at. The
 * Tasks screen refreshes every three seconds while a job runs, so a tab
 * left open in the background made twelve hundred requests an hour against a
 * machine that is usually also running Radarr, Sonarr and everything else the
 * household points at it.
 *
 * Coming back reloads immediately rather than waiting out an interval: the data
 * on screen is exactly as stale as the time spent away, and an interval later is
 * the one moment the user is certain to be reading it.
 *
 * Call it inside a component. `interval` and `active` are read reactively — pass
 * getters, not values — so a caller can change the pace or stop entirely and the
 * timer follows.
 */
export function poll(
  reload: () => void,
  interval: () => number,
  active: () => boolean = () => true,
) {
  $effect(() => {
    if (!active()) return;
    const every = interval();

    let timer: ReturnType<typeof setInterval> | undefined;

    const stop = () => {
      if (timer !== undefined) {
        clearInterval(timer);
        timer = undefined;
      }
    };
    const start = () => {
      stop();
      timer = setInterval(reload, every);
    };

    const onVisibility = () => {
      if (document.visibilityState === 'hidden') {
        stop();
        return;
      }
      reload();
      start();
    };

    if (document.visibilityState === 'visible') start();
    document.addEventListener('visibilitychange', onVisibility);

    return () => {
      stop();
      document.removeEventListener('visibilitychange', onVisibility);
    };
  });
}
