/** Imperative navigation for non-component code (upload flows, toasts). */
let navFn: ((to: string) => void) | null = null;

export function setNavigate(fn: (to: string) => void) {
  navFn = fn;
}

export function go(to: string) {
  navFn?.(to);
}
