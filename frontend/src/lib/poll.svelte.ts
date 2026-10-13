/**
 * Polls while the tab is in front, and reloads at once when it comes back.
 *
 * Call it inside a component. `interval` and `active` are getters read reactively, so the
 * timer follows a change of pace or a stop.
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
