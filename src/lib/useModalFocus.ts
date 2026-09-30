import { useEffect, useRef } from "react";

const stack: HTMLElement[] = [];
const inertOwners = new Map<HTMLElement, { count: number; original: boolean }>();
const focusable = 'button:not([disabled]), a[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** One modal lifecycle owner: contain focus, isolate background, and restore trigger. */
export function useModalFocus(open: boolean, onClose: () => void) {
  const ref = useRef<HTMLDivElement>(null);
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    const dialog = ref.current;
    if (!open || !dialog) return;
    const trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const isolated: HTMLElement[] = [];
    for (let node: HTMLElement = dialog; node.parentElement; node = node.parentElement) {
      for (const sibling of node.parentElement.children) {
        if (sibling === node || !(sibling instanceof HTMLElement)) continue;
        const owner = inertOwners.get(sibling) ?? { count: 0, original: sibling.inert };
        owner.count += 1;
        inertOwners.set(sibling, owner);
        sibling.inert = true;
        isolated.push(sibling);
      }
      if (node.parentElement === document.body) break;
    }
    stack.push(dialog);
    const items = () => Array.from(dialog.querySelectorAll<HTMLElement>(focusable))
      .filter((item) => !item.closest('[inert]') && item.getClientRects().length > 0);
    (items()[0] ?? dialog).focus();
    const keydown = (event: KeyboardEvent) => {
      if (stack[stack.length - 1] !== dialog) return;
      if (event.key === 'Escape') {
        event.preventDefault(); event.stopImmediatePropagation(); close.current();
      } else if (event.key === 'Tab') {
        const all = items();
        const target = event.shiftKey ? all[all.length - 1] : all[0];
        if (!all.length || !dialog.contains(document.activeElement) ||
            (event.shiftKey && document.activeElement === all[0]) ||
            (!event.shiftKey && document.activeElement === all[all.length - 1])) {
          event.preventDefault(); (target ?? dialog).focus();
        }
      }
    };
    const focusin = (event: FocusEvent) => {
      if (stack[stack.length - 1] === dialog && !dialog.contains(event.target as Node)) {
        (items()[0] ?? dialog).focus();
      }
    };
    document.addEventListener('keydown', keydown, true);
    document.addEventListener('focusin', focusin);
    return () => {
      document.removeEventListener('keydown', keydown, true);
      document.removeEventListener('focusin', focusin);
      stack.splice(stack.indexOf(dialog), 1);
      for (const node of isolated) {
        const owner = inertOwners.get(node)!;
        owner.count -= 1;
        if (!owner.count) { node.inert = owner.original; inertOwners.delete(node); }
      }
      if (trigger?.isConnected && !trigger.closest('[inert]')) trigger.focus();
    };
  }, [open]);
  return ref;
}
