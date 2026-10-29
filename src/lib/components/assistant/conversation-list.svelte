<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import {
		Delete02Icon,
		Edit02Icon,
		MessageMultiple01Icon,
		PlusSignIcon,
		RefreshIcon,
	} from "@hugeicons/core-free-icons";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { Input } from "$lib/components/ui/input/index.js";
	import type { Assistant } from "$lib/assistant/store.svelte.js";

	let {
		assistant,
		onselect,
	}: {
		assistant: Assistant;
		onselect?: () => void;
	} = $props();

	/** The conversation whose title is being edited, and the draft title. */
	let renaming = $state("");
	let draft = $state("");
	/** The conversation whose delete is being confirmed. */
	let confirming = $state("");

	/** A row is only actionable while no turn is running. */
	const frozen = $derived(assistant.busy || assistant.opening);

	function select(id: string) {
		if (frozen || renaming || confirming) return;
		void assistant.openConversation(id);
		onselect?.();
	}

	function startRename(id: string, title: string) {
		renaming = id;
		draft = title;
		confirming = "";
	}

	async function commitRename() {
		const id = renaming;
		if (!id) return;
		if (await assistant.renameConversation(id, draft)) {
			renaming = "";
			draft = "";
		}
	}

	function cancelRename() {
		renaming = "";
		draft = "";
	}

	async function remove(id: string) {
		if (await assistant.deleteConversation(id)) confirming = "";
	}

	/** A saved conversation's date, short enough for a sidebar row. */
	function when(iso: string): string {
		const date = new Date(iso);
		if (Number.isNaN(date.getTime())) return "";
		return date.toLocaleDateString(undefined, { month: "short", day: "numeric" });
	}
</script>

<div class="flex min-h-0 flex-1 flex-col">
	<div class="flex shrink-0 items-center gap-1 border-b px-2 py-1.5">
		<span class="text-xs font-medium">Conversations</span>
		<Button
			size="icon-sm"
			variant="ghost"
			class="ml-auto"
			disabled={frozen}
			aria-label="Refresh conversations"
			title="Refresh conversations"
			onclick={() => assistant.refreshSessions()}
		>
			<HugeiconsIcon icon={RefreshIcon} strokeWidth={2} />
		</Button>
		<Button
			size="icon-sm"
			variant="ghost"
			disabled={frozen}
			aria-label="New conversation"
			title="New conversation"
			onclick={() => {
				assistant.newConversation();
				onselect?.();
			}}
		>
			<HugeiconsIcon icon={PlusSignIcon} strokeWidth={2} />
		</Button>
	</div>

	<div class="min-h-0 flex-1 overflow-y-auto overscroll-contain p-1.5">
		{#if assistant.sessionsError}
			<Alert variant="destructive" class="mb-1.5">{assistant.sessionsError}</Alert>
		{/if}
		{#if assistant.malformed.length > 0}
			<div class="text-muted-foreground mb-1.5 px-1 text-[0.625rem]">
				{assistant.malformed.length} conversation file{assistant.malformed.length === 1
					? ""
					: "s"} could not be read.
				{#each assistant.malformed as file (file.path)}
					<div class="mb-1">
						<p class="text-destructive truncate font-mono" title={file.path}>{file.path}</p>
						<p class="text-destructive">{file.error}</p>
					</div>
				{/each}
			</div>
		{/if}

		{#if assistant.sessions.length === 0 && !assistant.sessionsError}
			<p class="text-muted-foreground px-1 py-2 text-[0.6875rem]">
				No conversations yet. The first message starts one inside the ledger
				(<code class="font-mono">.ledger/sessions/</code>).
			</p>
		{/if}

		<ul class="flex flex-col gap-0.5">
			{#each assistant.sessions as session (session.id)}
				<li>
					{#if confirming === session.id}
						<div class="bg-destructive/10 flex flex-col gap-1.5 rounded-md p-1.5">
							<p class="text-xs">
								Delete “{session.title}”? The file is removed; git history keeps the rest.
							</p>
							<div class="flex gap-1">
								<Button
									size="xs"
									variant="destructive"
									disabled={frozen}
									onclick={() => void remove(session.id)}>Delete</Button
								>
								<Button size="xs" variant="ghost" onclick={() => (confirming = "")}
									>Cancel</Button
								>
							</div>
						</div>
					{:else if renaming === session.id}
						<div class="flex flex-col gap-1 p-1">
							<Input
								bind:value={draft}
								class="h-7 text-xs"
								aria-label="Conversation title"
								onkeydown={(event) => {
									if (event.key === "Enter") {
										event.preventDefault();
										void commitRename();
									} else if (event.key === "Escape") {
										event.preventDefault();
										cancelRename();
									}
								}}
							/>
							<div class="flex gap-1">
								<Button size="xs" disabled={frozen || !draft.trim()} onclick={commitRename}
									>Save</Button
								>
								<Button size="xs" variant="ghost" onclick={cancelRename}>Cancel</Button>
							</div>
						</div>
					{:else}
						<div
							class="group flex items-center gap-0.5 rounded-md {assistant.activeId ===
							session.id
								? 'bg-muted'
								: 'hover:bg-muted/60'}"
						>
							<button
								type="button"
								class="flex min-w-0 flex-1 flex-col items-start gap-0.5 px-1.5 py-1 text-left disabled:opacity-50"
								disabled={frozen}
								title={session.title}
								onclick={() => select(session.id)}
							>
								<span class="w-full truncate text-xs">{session.title}</span>
								<span class="text-muted-foreground flex items-center gap-1 text-[0.625rem]">
									<HugeiconsIcon icon={MessageMultiple01Icon} strokeWidth={2} class="size-2.5" />
									{session.messages}
									<span aria-hidden="true">·</span>
									{when(session.updated)}
								</span>
							</button>
							<Button
								size="icon-xs"
								variant="ghost"
								class="text-muted-foreground opacity-70 transition-opacity md:opacity-0 md:group-hover:opacity-100 focus-visible:opacity-100"
								disabled={frozen}
								aria-label="Rename {session.title}"
								onclick={() => startRename(session.id, session.title)}
							>
								<HugeiconsIcon icon={Edit02Icon} strokeWidth={2} />
							</Button>
							<Button
								size="icon-xs"
								variant="ghost"
								class="text-muted-foreground opacity-70 transition-opacity md:opacity-0 md:group-hover:opacity-100 focus-visible:opacity-100"
								disabled={frozen}
								aria-label="Delete {session.title}"
								onclick={() => {
									confirming = session.id;
									renaming = "";
								}}
							>
								<HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
							</Button>
						</div>
					{/if}
				</li>
			{/each}
		</ul>
	</div>
</div>
