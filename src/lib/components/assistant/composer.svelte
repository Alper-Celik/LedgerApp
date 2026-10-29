<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import { Attachment01Icon, ArrowUp01Icon, Cancel01Icon } from "@hugeicons/core-free-icons";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { Input } from "$lib/components/ui/input/index.js";
	import { Textarea } from "$lib/components/ui/textarea/index.js";
	import {
		ATTACHMENT_ACCEPT,
		MAX_ATTACHMENTS,
		MAX_ATTACHMENT_BYTES,
		formatBytes,
		readFiles,
		visionOf,
		type Attachment,
	} from "$lib/assistant/media.js";
	import type { Assistant } from "$lib/assistant/store.svelte.js";

	let { assistant }: { assistant: Assistant } = $props();

	let text = $state("");
	let attachments = $state<Attachment[]>([]);
	let fileError = $state("");
	let fileInput = $state<HTMLInputElement | null>(null);
	let textarea = $state<HTMLElement | null>(null);

	const canSend = $derived(
		!assistant.busy && !assistant.opening && (text.trim().length > 0 || attachments.length > 0)
	);

	/** The endpoint's own answer about the selected model; unknown is not a yes. */
	const vision = $derived(visionOf(assistant.models, assistant.settings?.model ?? ""));

	const attachmentNotice = $derived(
		attachments.length === 0 || vision === "yes"
			? ""
			: vision === "no"
				? `The endpoint reports that ${assistant.settings?.model || "this model"} does not accept images: these files will not be sent.`
				: `The endpoint did not say whether ${assistant.settings?.model || "this model"} accepts images: these files will not be sent.`
	);

	async function chooseFiles(event: Event) {
		const input = event.currentTarget as HTMLInputElement;
		const files = Array.from(input.files ?? []);
		input.value = "";
		if (files.length === 0) return;
		fileError = "";
		const read = await readFiles(files, attachments);
		attachments = [...attachments, ...read.attachments];
		fileError = read.errors.join(" ");
	}

	function removeAttachment(index: number) {
		attachments = attachments.filter((_, at) => at !== index);
		fileError = "";
	}

	async function send() {
		if (!canSend) return;
		const outgoing = text;
		const files = attachments;
		const started = await assistant.send(outgoing, files);
		if (!started) return;
		// A draft typed while the turn ran is not the message just sent.
		if (text === outgoing) text = "";
		if (attachments === files) {
			attachments = [];
			fileError = "";
		}
		textarea?.focus();
	}

	function onKeydown(event: KeyboardEvent) {
		if (event.key !== "Enter" || event.shiftKey || event.isComposing) return;
		event.preventDefault();
		void send();
	}
</script>

<div
	class="bg-background/95 supports-[backdrop-filter]:bg-background/80 shrink-0 border-t px-2 pt-2 backdrop-blur"
	style="padding-bottom: max(0.5rem, env(safe-area-inset-bottom))"
>
	<div class="mx-auto flex w-full max-w-3xl flex-col gap-1.5">
		{#if attachments.length > 0}
			<div class="flex flex-wrap items-start gap-1">
				{#each attachments as attachment, index (attachment.name + index)}
					<div
						class="bg-muted flex max-w-full flex-col gap-0.5 rounded-md px-1.5 py-0.5 text-[0.6875rem]"
					>
						<span class="flex items-center gap-1">
							<span class="max-w-40 truncate" title={attachment.name}>{attachment.name}</span>
							<span class="text-muted-foreground">{formatBytes(attachment.size)}</span>
							<Button
								size="icon-xs"
								variant="ghost"
								aria-label="Remove {attachment.name}"
								onclick={() => removeAttachment(index)}
							>
								<HugeiconsIcon icon={Cancel01Icon} strokeWidth={2} />
							</Button>
						</span>
						{#if attachment.mime === "application/pdf"}
							<!--
								Where the PDF sits in the ledger: the page router only needs a
								path inside the ledger's own directory, and the picker never
								says which folder the file came from. Empty means "send the
								browser's bytes instead", the only route a PDF outside the
								ledger has.
							-->
							<Input
								class="h-6 w-52 max-w-full px-1 text-[0.6875rem]"
								value={attachment.path ?? ""}
								placeholder="path in the ledger (empty: send as is)"
								aria-label="Path of {attachment.name} relative to the ledger"
								oninput={(event) => {
									attachment.path = event.currentTarget.value;
								}}
							/>
						{/if}
					</div>
				{/each}
			</div>
		{/if}

		{#if attachmentNotice}
			<Alert>{attachmentNotice}</Alert>
		{/if}
		{#if fileError}
			<Alert variant="destructive">{fileError}</Alert>
		{/if}

		<div class="flex items-end gap-1">
			<Button
				size="icon-sm"
				variant="ghost"
				class="shrink-0"
				disabled={assistant.busy || attachments.length >= MAX_ATTACHMENTS}
				aria-label="Attach files"
				title="Attach images, PDFs or CSVs (up to {MAX_ATTACHMENTS} files, {formatBytes(
					MAX_ATTACHMENT_BYTES
				)} each)"
				onclick={() => fileInput?.click()}
			>
				<HugeiconsIcon icon={Attachment01Icon} strokeWidth={2} />
			</Button>
			<input
				bind:this={fileInput}
				class="hidden"
				type="file"
				multiple
				accept={ATTACHMENT_ACCEPT}
				onchange={chooseFiles}
			/>
			<Textarea
				bind:ref={textarea}
				bind:value={text}
				rows={1}
				class="max-h-32 min-h-9 flex-1 py-1.5"
				placeholder="Ask about the ledger…"
				aria-label="Message"
				onkeydown={onKeydown}
			></Textarea>
			<Button
				size="icon-sm"
				class="shrink-0"
				disabled={!canSend}
				aria-label="Send"
				onclick={() => void send()}
			>
				<HugeiconsIcon icon={ArrowUp01Icon} strokeWidth={2} />
			</Button>
		</div>
		<p class="text-muted-foreground hidden px-1 text-[0.625rem] sm:block">
			Enter sends · Shift+Enter starts a new line
		</p>
	</div>
</div>
