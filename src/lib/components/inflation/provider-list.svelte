<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import type { IndexProviderInfo } from "../../../bindings";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Badge } from "$lib/components/ui/badge/index.js";
	import { Button } from "$lib/components/ui/button/index.js";

	/**
	 * The publishers this build can build a series from, minus the ones already
	 * stored — the way to add a series the reader does not have yet. A provider
	 * with no public download endpoint (TÜİK's signed links, ENAG's table) is
	 * listed with its explanation, and its button opens the import sheet: the
	 * note says why there is nothing to fetch, and the sheet is where its CSV
	 * goes.
	 */
	let {
		providers,
		stored,
		fetching,
		errors,
		onfetch,
		onimport,
	}: {
		providers: IndexProviderInfo[];
		/** The ids the index store already holds. */
		stored: string[];
		/** Per-provider fetch in flight, by id. */
		fetching: Record<string, boolean>;
		/** Per-provider refusal, by id, verbatim. */
		errors: Record<string, string>;
		onfetch: (provider: IndexProviderInfo) => void;
		/** Open the import sheet, for a provider with no download endpoint. */
		onimport: () => void;
	} = $props();

	const pending = $derived(providers.filter((provider) => !stored.includes(provider.id)));
</script>

{#if providers.length === 0}
	<p class="text-muted-foreground text-xs">
		This build knows no index publisher to fetch from.
	</p>
{:else if pending.length === 0}
	<p class="text-muted-foreground text-xs">
		Every publisher this build knows is already stored — refresh a series on its card to ask for
		its newest months.
	</p>
{:else}
	<ul class="divide-border divide-y rounded-lg border">
		{#each pending as provider (provider.id)}
			<li class="flex flex-col gap-1.5 px-3 py-2">
				<div class="flex flex-wrap items-center gap-x-2 gap-y-1">
					<span class="text-sm font-medium">{provider.label}</span>
					{#if provider.unofficial}
						<Badge
							variant="destructive"
							title="Published by an independent body, not a national statistical agency"
						>
							unofficial
						</Badge>
					{/if}
					<span class="text-muted-foreground font-mono text-[0.625rem]">{provider.id}</span>
					<span class="text-muted-foreground text-xs">{provider.source} · {provider.currency}</span>
					{#if provider.downloadable}
						<Button
							class="ms-auto h-8"
							variant="secondary"
							disabled={fetching[provider.id] === true}
							title={`Download ${provider.label} and store it`}
							aria-label={`Fetch ${provider.label} from ${provider.source}`}
							onclick={() => onfetch(provider)}
						>
							{fetching[provider.id] ? "Fetching…" : "Fetch"}
						</Button>
					{:else}
						<!--
							No endpoint to download: the sheet is the only way in, and
							this button is the way to it, so the row is not a dead end.
						-->
						<Button
							class="ms-auto h-8"
							variant="outline"
							title={provider.note}
							aria-label={`Import ${provider.label} from a CSV export`}
							onclick={onimport}
						>
							Import CSV
						</Button>
					{/if}
				</div>
				{#if !provider.downloadable}
					<!-- Verbatim: the publisher's own reason for having no download. -->
					<p class="text-muted-foreground text-xs">{provider.note}</p>
				{/if}
				{#if errors[provider.id]}
					<Alert variant="destructive">{errors[provider.id]}</Alert>
				{/if}
			</li>
		{/each}
	</ul>
{/if}
