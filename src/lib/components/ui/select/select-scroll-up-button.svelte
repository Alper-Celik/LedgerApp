<!--
SPDX-FileCopyrightText: 2026 shadcn-svelte contributors <https://github.com/huntabyte/shadcn-svelte>

SPDX-License-Identifier: MIT
-->
<script lang="ts">
	import { Select as SelectPrimitive } from "bits-ui";
	import { HugeiconsIcon } from "@hugeicons/svelte"
	import { ArrowUp01Icon } from '@hugeicons/core-free-icons';
	import { cn, type WithoutChildrenOrChild } from "$lib/utils.js";

	let {
		ref = $bindable(null),
		class: className,
		...restProps
	}: WithoutChildrenOrChild<SelectPrimitive.ScrollUpButtonProps> = $props();
</script>

<SelectPrimitive.ScrollUpButton
	bind:ref
	data-slot="select-scroll-up-button"
	class={cn("bg-popover z-10 flex cursor-default items-center justify-center py-1 [&_svg:not([class*='size-'])]:size-3.5", className)}
	{...restProps}
>
	<HugeiconsIcon icon={ArrowUp01Icon} strokeWidth={2}  />
</SelectPrimitive.ScrollUpButton>
