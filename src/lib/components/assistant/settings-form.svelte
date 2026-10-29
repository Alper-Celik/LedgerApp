<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { untrack } from "svelte";
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import { RefreshIcon, Tick01Icon } from "@hugeicons/core-free-icons";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { Input } from "$lib/components/ui/input/index.js";
	import * as Select from "$lib/components/ui/select/index.js";
	import type { Assistant } from "$lib/assistant/store.svelte.js";

	/**
	 * The endpoint form and the model picker.
	 *
	 * `gate` is first run: once a key is saved the panel stays here, on the
	 * model picker, until "Start chatting" is pressed. Without `gate` this is the
	 * settings editor, where saving keeps the stored key when the key field is
	 * left blank.
	 */
	let { assistant, gate = false }: { assistant: Assistant; gate?: boolean } = $props();

	// The form is a draft seeded from the saved settings once; the store's
	// settings are not the draft. `untrack` says that on purpose.
	let baseUrl = $state(untrack(() => assistant.settings?.base_url ?? "https://api.openai.com/v1"));
	let model = $state(untrack(() => assistant.settings?.model ?? ""));
	let apiKey = $state("");
	let saving = $state(false);
	/** Gate: false once the key is saved, so the model picker takes over. */
	let showForm = $state(untrack(() => !gate || !assistant.settings?.has_key));

	const configured = $derived(assistant.settings?.has_key === true);
	const canSave = $derived(
		!saving &&
			baseUrl.trim().length > 0 &&
			(configured ? true : apiKey.trim().length > 0)
	);
	const selectedModel = $derived(assistant.settings?.model ?? "");
	const selectedInfo = $derived(
		assistant.models.find((candidate) => candidate.id === selectedModel) ?? null
	);

	async function save() {
		if (!canSave) return;
		saving = true;
		const ok = await assistant.configure(
			baseUrl.trim(),
			apiKey.trim() ? apiKey.trim() : null,
			model.trim(),
			assistant.settings?.max_tokens ?? null,
			assistant.settings?.temperature ?? null
		);
		saving = false;
		if (!ok) return;
		apiKey = "";
		if (assistant.settings) {
			baseUrl = assistant.settings.base_url;
			model = assistant.settings.model;
		}
		if (gate) showForm = false;
	}

	async function choose(next: string) {
		if (!next) return;
		if (await assistant.selectModel(next)) model = next;
	}
</script>

<div class="flex flex-col gap-3">
	<div class="flex flex-col gap-0.5">
		<h2 class="text-sm font-medium">
			{gate ? "Connect a model" : "Endpoint"}
		</h2>
		<p class="text-muted-foreground text-xs">
			An OpenAI-compatible API root — OpenAI, Ollama, llama.cpp, vLLM, OpenRouter or a
			gateway. The key is written by the backend to its own private config file; the browser
			never stores it.
		</p>
	</div>

	{#if gate && configured && !showForm}
		<div class="flex flex-col gap-2">
			<div class="flex items-center gap-1.5 text-xs">
				<HugeiconsIcon icon={Tick01Icon} strokeWidth={2} class="text-primary size-3.5" />
				<span>Saved <code class="font-mono">{assistant.settings?.base_url}</code>.</span>
			</div>
			<p class="text-muted-foreground text-xs">Pick the model the panel should ask.</p>

			{#if assistant.modelsLoading}
				<p class="text-muted-foreground text-xs">Asking the endpoint for its models…</p>
			{:else if assistant.modelsError}
				<Alert variant="destructive">{assistant.modelsError}</Alert>
				<Button size="sm" variant="secondary" onclick={() => assistant.loadModels()}>
					Try again
				</Button>
			{:else if assistant.models.length === 0}
				<p class="text-muted-foreground text-xs">
					The endpoint listed no models. The model saved above is kept; start chatting if it
					is the one you want.
				</p>
			{:else}
				<Select.Root
					type="single"
					value={selectedModel}
					onValueChange={(next) => {
						if (next) void choose(next);
					}}
				>
					<Select.Trigger class="w-full" aria-label="Model">
						<span data-slot="select-value">
							{selectedModel || "Choose a model"}
						</span>
					</Select.Trigger>
					<Select.Content>
						{#each assistant.models as candidate (candidate.id)}
							<Select.Item
								value={candidate.id}
								label={candidate.vision ? `${candidate.id} · vision` : candidate.id}
							/>
						{/each}
					</Select.Content>
				</Select.Root>
			{/if}

			<div class="flex flex-wrap items-center gap-2">
				<Button
					size="lg"
					class="w-full sm:w-auto"
					disabled={!assistant.settings?.model}
					onclick={() => assistant.finishSetup()}
				>
					Start chatting
				</Button>
				<Button size="sm" variant="ghost" onclick={() => (showForm = true)}>
					Change endpoint
				</Button>
			</div>
			{#if selectedInfo && selectedInfo.vision !== true}
				<p class="text-muted-foreground text-[0.6875rem]">
					The endpoint {selectedInfo.vision === false
						? "says this model does not accept images"
						: "did not say whether this model accepts images"}; attachments will not be
					sent.
				</p>
			{/if}
		</div>
	{:else}
		<form
			class="flex flex-col gap-2"
			onsubmit={(event) => {
				event.preventDefault();
				void save();
			}}
		>
			<label class="flex flex-col gap-1">
				<span class="text-muted-foreground text-xs">Base URL</span>
				<Input
					type="url"
					bind:value={baseUrl}
					placeholder="https://api.openai.com/v1"
					autocomplete="off"
					disabled={saving}
				/>
			</label>
			<label class="flex flex-col gap-1">
				<span class="text-muted-foreground text-xs">API key</span>
				<Input
					type="password"
					bind:value={apiKey}
					placeholder={configured ? "leave blank to keep the stored key" : "sk-…"}
					autocomplete="off"
					disabled={saving}
				/>
				<span class="text-muted-foreground text-[0.625rem]">
					Stored by the backend in its private config file (mode 0600). A local endpoint that
					ignores the key can take any placeholder.
				</span>
			</label>
			<label class="flex flex-col gap-1">
				<span class="text-muted-foreground text-xs">Model</span>
				<Input
					bind:value={model}
					placeholder="e.g. gpt-4o — or leave blank and pick one after saving"
					autocomplete="off"
					disabled={saving}
				/>
			</label>

			{#if assistant.settingsError}
				<Alert variant="destructive">{assistant.settingsError}</Alert>
			{/if}

			<div class="flex flex-wrap items-center gap-2">
				<Button type="submit" size="lg" class="w-full sm:w-auto" disabled={!canSave}>
					{saving ? "Saving…" : configured ? "Save settings" : "Save and continue"}
				</Button>
				{#if gate && configured}
					<Button size="sm" variant="ghost" onclick={() => (showForm = false)}>
						Back to models
					</Button>
				{/if}
			</div>
		</form>

		{#if configured}
			<div class="flex flex-col gap-2 border-t pt-3">
				<div class="flex items-center gap-1">
					<span class="text-xs font-medium">Model</span>
					<Button
						size="icon-xs"
						variant="ghost"
						class="ml-auto"
						aria-label="Refresh models"
						title="Refresh models"
						disabled={assistant.modelsLoading}
						onclick={() => assistant.loadModels()}
					>
						<HugeiconsIcon
							icon={RefreshIcon}
							strokeWidth={2}
							class={assistant.modelsLoading ? "animate-spin" : ""}
						/>
					</Button>
				</div>
				{#if assistant.modelsError}
					<Alert variant="destructive">{assistant.modelsError}</Alert>
				{:else if assistant.modelsLoading && assistant.models.length === 0}
					<p class="text-muted-foreground text-xs">Asking the endpoint for its models…</p>
				{:else if assistant.models.length === 0}
					<p class="text-muted-foreground text-xs">
						The endpoint listed no models; the typed model is kept.
					</p>
				{:else}
					<Select.Root
						type="single"
						value={selectedModel}
						onValueChange={(next) => {
							if (next) void choose(next);
						}}
					>
						<Select.Trigger class="w-full" aria-label="Model">
							<span data-slot="select-value">
								{selectedModel || "Choose a model"}
							</span>
						</Select.Trigger>
						<Select.Content>
							{#each assistant.models as candidate (candidate.id)}
								<Select.Item
									value={candidate.id}
									label={candidate.vision ? `${candidate.id} · vision` : candidate.id}
								/>
							{/each}
						</Select.Content>
					</Select.Root>
				{/if}
				{#if selectedInfo}
					<p class="text-muted-foreground text-[0.6875rem]">
						{selectedInfo.context_window
							? `${selectedInfo.context_window.toLocaleString()} token context · `
							: ""}{selectedInfo.vision === true
							? "accepts images"
							: selectedInfo.vision === false
								? "does not accept images"
								: "vision support not reported"}.
					</p>
				{/if}
			</div>
		{/if}
	{/if}
</div>
