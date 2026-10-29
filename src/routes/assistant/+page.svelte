<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<!--
	The ledger's assistant.

	Full width below `md` (where the app sidebar is a sheet), with the transcript
	as the only scroller and the composer pinned to the bottom; the height follows
	`visualViewport` so the composer stays above the on-screen keyboard. Desktop
	keeps the conversation list beside the transcript.
-->
<script lang="ts">
	import { onMount } from "svelte";
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import {
		Menu01Icon,
		Settings01Icon,
		SparklesIcon,
	} from "@hugeicons/core-free-icons";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import * as Sheet from "$lib/components/ui/sheet/index.js";
	import ConversationList from "$lib/components/assistant/conversation-list.svelte";
	import SettingsForm from "$lib/components/assistant/settings-form.svelte";
	import MessageList from "$lib/components/assistant/message-list.svelte";
	import Composer from "$lib/components/assistant/composer.svelte";
	import { assistant } from "$lib/assistant/store.svelte.js";
	import { ledgerEpoch } from "$lib/ledger-revision.svelte.js";

	let mobileList = $state(false);
	let settingsOpen = $state(false);
	/** The visual viewport's height, so the composer sits above a phone keyboard. */
	let viewportHeight = $state<number | null>(null);

	const needsSetup = $derived(
		assistant.setupOpen ||
			(assistant.settings !== null &&
				(!assistant.settings.has_key || assistant.settings.model.trim() === ""))
	);

	$effect(() => {
		// Another ledger is open: the conversation and the tool reads it drives
		// would all point at the previous one, so start clean.
		assistant.adoptLedger(ledgerEpoch.value);
	});

	onMount(() => {
		void assistant.init();

		const viewport = window.visualViewport;
		if (!viewport) return;
		const update = () => {
			const mobile = window.matchMedia("(max-width: 767px)").matches;
			// The app's own header (h-11) sits above this page below `md`.
			viewportHeight = Math.max(160, Math.round(viewport.height - (mobile ? 44 : 0)));
		};
		update();
		viewport.addEventListener("resize", update);
		window.addEventListener("resize", update);
		return () => {
			viewport.removeEventListener("resize", update);
			window.removeEventListener("resize", update);
		};
	});
</script>

<div
	class="flex h-[calc(100dvh-2.75rem)] w-full flex-col overflow-hidden md:h-dvh"
	style:height={viewportHeight !== null ? `${viewportHeight}px` : undefined}
>
	<header class="flex h-10 shrink-0 items-center gap-1 border-b px-2">
		<Button
			size="icon-sm"
			variant="ghost"
			class="md:hidden"
			aria-label="Conversations"
			onclick={() => (mobileList = true)}
		>
			<HugeiconsIcon icon={Menu01Icon} strokeWidth={2} />
		</Button>
		<HugeiconsIcon icon={SparklesIcon} strokeWidth={2} class="text-muted-foreground size-3.5" />
		<span class="min-w-0 flex-1 truncate text-sm font-medium">
			{needsSetup ? "Assistant setup" : assistant.activeTitle || "New conversation"}
		</span>
		{#if assistant.settings?.model}
			<span
				class="text-muted-foreground hidden max-w-40 truncate font-mono text-[0.625rem] sm:inline"
				title={assistant.settings.model}
			>
				{assistant.settings.model}
			</span>
		{/if}
		{#if !needsSetup && assistant.settings}
			<Button
				size="icon-sm"
				variant="ghost"
				aria-label="Assistant settings"
				title="Assistant settings"
				onclick={() => (settingsOpen = true)}
			>
				<HugeiconsIcon icon={Settings01Icon} strokeWidth={2} />
			</Button>
		{/if}
	</header>

	<div class="flex min-h-0 flex-1">
		<aside class="hidden min-h-0 w-60 shrink-0 flex-col border-r md:flex">
			<ConversationList {assistant} />
		</aside>

		<section class="flex min-h-0 min-w-0 flex-1 flex-col">
			{#if assistant.loading}
				<div class="text-muted-foreground flex flex-1 items-center justify-center gap-2 text-xs">
					<span
						class="size-3 animate-spin rounded-full border-2 border-current border-t-transparent"
					></span>
					Reading the assistant's settings…
				</div>
			{:else if assistant.settingsError && !assistant.settings}
				<div class="flex flex-1 flex-col justify-center gap-2 p-4">
					<Alert variant="destructive">{assistant.settingsError}</Alert>
					<Button size="sm" variant="secondary" onclick={() => assistant.retrySettings()}>
						Try again
					</Button>
				</div>
			{:else if needsSetup}
				<div class="min-h-0 flex-1 overflow-y-auto p-4">
					<div class="mx-auto w-full max-w-md">
						<SettingsForm {assistant} gate />
					</div>
				</div>
			{:else}
				{#if assistant.toolsError}
					<div class="flex shrink-0 flex-col gap-1 border-b px-3 py-1.5">
						<Alert variant="destructive">
							<span class="font-medium">Ledger tools could not be loaded:</span>
							{assistant.toolsError}
						</Alert>
						<Button
							size="xs"
							variant="secondary"
							class="self-start"
							onclick={() => assistant.loadTools()}
						>
							Try again
						</Button>
					</div>
				{:else if assistant.tools.length === 0}
					<div class="text-muted-foreground shrink-0 border-b px-3 py-1.5 text-[0.6875rem]">
						The backend offers no ledger tools in this session, so answers cannot read the
						ledger's figures.
					</div>
				{/if}
				<MessageList {assistant} />
				<Composer {assistant} />
			{/if}
		</section>
	</div>
</div>

<Sheet.Root bind:open={mobileList}>
	<Sheet.Content side="left" class="w-72 max-w-[85vw] gap-0 p-0">
		<Sheet.Header class="border-b px-3 py-2">
			<Sheet.Title class="text-sm">Conversations</Sheet.Title>
		</Sheet.Header>
		<div class="flex min-h-0 flex-1 flex-col">
			<ConversationList {assistant} onselect={() => (mobileList = false)} />
		</div>
	</Sheet.Content>
</Sheet.Root>

<Sheet.Root bind:open={settingsOpen}>
	<Sheet.Content
		side="right"
		class="data-[side=right]:w-full gap-0 data-[side=right]:sm:max-w-md"
	>
		<Sheet.Header>
			<Sheet.Title>Assistant</Sheet.Title>
			<Sheet.Description>
				The endpoint, key and model. The key lives in the backend's private config file, never
				in the browser.
			</Sheet.Description>
		</Sheet.Header>
		<div class="min-h-0 flex-1 overflow-y-auto p-4">
			<SettingsForm {assistant} />
		</div>
	</Sheet.Content>
</Sheet.Root>
