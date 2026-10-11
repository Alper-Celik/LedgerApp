// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * Svelte action: animate a container's own height whenever its content changes
 * it, so lists grow and fold smoothly instead of snapping. Driven by a
 * ResizeObserver — the callback runs after layout — so it needs no knowledge of
 * what changed: rows appearing, a filter narrowing, a fold opening.
 *
 * Duration and easing come from the theme (`--motion-duration`,
 * `--motion-ease` — Material 3's emphasized curve by default), so this module
 * carries no numbers of its own.
 */
export function smoothHeight(node: HTMLElement) {
	let previous = node.getBoundingClientRect().height;
	// "240ms" or "0.24s" — whatever the theme says, in milliseconds.
	const readMs = () => {
		const raw = getComputedStyle(node).getPropertyValue("--motion-duration").trim();
		const value = Number.parseFloat(raw);
		if (!Number.isFinite(value)) return 240;
		return raw.endsWith("ms") ? value : value * 1000;
	};
	const readEase = () => getComputedStyle(node).getPropertyValue("--motion-ease").trim() || "ease-out";
	const observer = new ResizeObserver(() => {
		const next = node.getBoundingClientRect().height;
		if (next === previous) return;
		const from = previous;
		previous = next;
		if (typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches) return;
		// Clipping vertically is what makes the growth read as the content opening.
		node.style.overflowY = "hidden";
		// Hold the old height until the animation's first frame lands, or the full
		// new height paints for one frame first.
		node.style.height = `${from}px`;
		// Stop observing while animating the node itself: otherwise every frame of
		// our own animation looks like another content change and they fight.
		observer.unobserve(node);
		node
			.animate([{ height: `${from}px` }, { height: `${next}px` }], {
				duration: readMs(),
				easing: readEase(),
			})
			.finished.then(() => {
				node.style.overflowY = "";
				node.style.height = "";
				previous = node.getBoundingClientRect().height;
			})
			.catch(() => {})
			.finally(() => observer.observe(node));
	});
	observer.observe(node);
	return { destroy: () => observer.disconnect() };
}
