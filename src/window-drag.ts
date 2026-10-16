/** Native dragging starts on mouse-down, as required by desktop compositors.
 * Only header background/handle/title can drag; controls retain their actions. */
export function installPinnedDragging(
  root: HTMLElement,
  pinned: () => boolean,
  startDragging: () => Promise<void>,
): () => void {
  const down = (event: MouseEvent) => {
    if (!pinned() || event.button !== 0 || !(event.target instanceof Element)) return;
    const target = event.target;
    if (!target.closest(".header") || target.closest("button, a, input, select, textarea, [data-action]")) return;
    event.preventDefault();
    void startDragging().catch(() => {});
  };
  root.addEventListener("mousedown", down);
  return () => root.removeEventListener("mousedown", down);
}
