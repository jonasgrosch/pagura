// @ts-check
/** @import { OutlineItem, Target } from './reading_protocol.mjs' */
/** Flat PDF preorder, visible ARIA treeitems and roving keyboard focus. */
export class OutlineTree {
  /**
   * @param {HTMLElement} root The role=tree container.
   * @param {(target: Target) => void} activate
   */
  constructor(root, activate) {
    this.root = root;
    this.activate = activate;
    /** @type {OutlineItem[]} */
    this.items = [];
    /** @type {Set<number>} Indexes of expanded items. */
    this.open = new Set();
    this.focus = 0;
  }
  /** @param {OutlineItem[]} items In PDF preorder. */
  set(items) {
    if (items.length > 20000) throw new Error('Outline exceeds the 20000-item DOM budget');
    this.items = items;
    this.open = new Set(items.flatMap((item, i) => (item.open ? [i] : [])));
    this.focus = 0;
    // Compute parent/subtree boundaries once; expanding never walks ancestors.
    /** @type {number[]} */
    const stack = [];
    /** @type {number[]} Parent index per item, -1 at the root. */
    this.parent = [];
    /** @type {number[]} Index after each item's subtree. */
    this.ends = items.map(() => items.length);
    items.forEach((item, i) => {
      while (stack.length > item.depth) this.ends[/** @type {number} */ (stack.pop())] = i;
      this.parent[i] = stack.at(-1) ?? -1;
      stack.push(i);
    });
    /** @type {Map<number, number>} */
    const totals = new Map(),
      ordinals = new Map();
    /** @type {number[]} */
    this.positions = [];
    this.parent.forEach((parent, i) => {
      totals.set(parent, (totals.get(parent) ?? 0) + 1);
      const n = (ordinals.get(parent) ?? 0) + 1;
      ordinals.set(parent, n);
      this.positions[i] = n;
    });
    /** @type {number[]} */
    this.sizes = this.parent.map((parent) => /** @type {number} */ (totals.get(parent)));
    this.render();
  }
  render(focus = false) {
    this.root.replaceChildren();
    /** @type {Map<number, HTMLElement>} */
    this.nodes = new Map();
    /** @type {number[]} Indexes of the rendered (visible) items. */
    this.visible = [];
    for (let i = 0; i < this.items.length; ) {
      const index = i,
        item = this.items[i],
        branch = this.ends[i] > i + 1;
      const node = document.createElement('div');
      node.setAttribute('role', 'treeitem');
      node.setAttribute('aria-level', String(item.depth + 1));
      node.setAttribute('aria-posinset', String(this.positions[i]));
      node.setAttribute('aria-setsize', String(this.sizes[i]));
      node.textContent = item.title;
      node.setAttribute('tabindex', i === this.focus ? '0' : '-1');
      if (branch) node.setAttribute('aria-expanded', String(this.open.has(i)));
      Object.assign(node.style, {
        paddingLeft: `${item.depth + 0.5}rem`,
        fontWeight: item.style & 2 ? 'bold' : 'normal',
        fontStyle: item.style & 1 ? 'italic' : 'normal',
        color: `rgb(${item.color.map((v) => Math.round(v * 255)).join(',')})`,
      });
      if (branch) {
        const toggle = document.createElement('span');
        toggle.textContent = this.open.has(index) ? '▾ ' : '▸ ';
        toggle.style.float = 'left';
        toggle.setAttribute('aria-hidden', 'true');
        toggle.addEventListener('click', (event) => {
          event.stopPropagation?.();
          this.focus = index;
          this.open.has(index) ? this.open.delete(index) : this.open.add(index);
          this.render(true);
        });
        node.append(toggle);
      }
      node.addEventListener('click', () => {
        this.focus = index;
        this.render(true);
        this.activate(item.target);
      });
      node.addEventListener('keydown', (event) => this.key(event, index, branch));
      this.root.append(node);
      this.nodes.set(i, node);
      this.visible.push(i);
      i = branch && !this.open.has(i) ? this.ends[i] : i + 1;
    }
    if (focus) this.nodes.get(this.focus)?.focus();
  }
  /**
   * @param {KeyboardEvent} event
   * @param {number} index
   * @param {boolean} branch
   */
  key(event, index, branch) {
    event.stopPropagation?.();
    const at = this.visible.indexOf(index);
    let next = index;
    if (event.key === 'ArrowDown') next = this.visible[Math.min(this.visible.length - 1, at + 1)];
    else if (event.key === 'ArrowUp') next = this.visible[Math.max(0, at - 1)];
    else if (event.key === 'Home') next = this.visible[0];
    else if (event.key === 'End') next = /** @type {number} */ (this.visible.at(-1));
    else if (event.key === 'ArrowRight' && branch) {
      if (this.open.has(index)) next = index + 1;
      else this.open.add(index);
    } else if (event.key === 'ArrowLeft') {
      if (branch && this.open.has(index)) this.open.delete(index);
      else next = this.parent[index] < 0 ? index : this.parent[index];
    } else if (event.key === 'Enter') {
      event.preventDefault();
      this.activate(this.items[index].target);
      return;
    } else if (event.key === ' ' && branch) {
      this.open.has(index) ? this.open.delete(index) : this.open.add(index);
    } else if (event.key.length === 1 && !event.ctrlKey && !event.metaKey) {
      const now = performance.now();
      this.prefix =
        now - (this.typedAt ?? 0) < 700 ? (this.prefix ?? '') + event.key.toLowerCase() : event.key.toLowerCase();
      this.typedAt = now;
      const prefix = this.prefix;
      next =
        this.visible
          .slice(at + 1)
          .concat(this.visible.slice(0, at + 1))
          .find((i) => this.items[i].title.toLowerCase().startsWith(prefix)) ?? index;
    } else return;
    event.preventDefault();
    this.focus = next;
    this.render(true);
  }
}
