// SPDX-FileCopyrightText: 2026 shadcn-svelte contributors <https://github.com/huntabyte/shadcn-svelte>
//
// SPDX-License-Identifier: MIT
import Content from "./tooltip-content.svelte";
import Portal from "./tooltip-portal.svelte";
import Provider from "./tooltip-provider.svelte";
import Trigger from "./tooltip-trigger.svelte";
import Root from "./tooltip.svelte";

export {
	Root,
	Trigger,
	Content,
	Provider,
	Portal,
	//
	Root as Tooltip,
	Content as TooltipContent,
	Trigger as TooltipTrigger,
	Provider as TooltipProvider,
	Portal as TooltipPortal,
};
