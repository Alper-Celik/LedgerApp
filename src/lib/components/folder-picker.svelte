<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { HugeiconsIcon } from "@hugeicons/svelte";
	import { ArrowUp01Icon, Folder01Icon, FolderCheckIcon } from "@hugeicons/core-free-icons";
	import { commands, type FolderListing } from "../../bindings";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import * as Sheet from "$lib/components/ui/sheet/index.js";
	import { closeFolderPicker, ledger, openLedgerPath } from "$lib/ledger.svelte.js";
	import { messageOf } from "$lib/errors.js";
	import { cn } from "$lib/utils.js";

	/**
	 * The in-app folder picker: a sheet that walks the filesystem one directory
	 * at a time and opens the folder it is standing in.
	 *
	 * The backend lists the folders (`list_folders`) and flags the ones holding
	 * a root journal, so the tree itself says where a ledger starts. It is tried
	 * after `pick_ledger_folder` (`chooseLedger` in `ledger.svelte.ts`), which is
	 * the normal way in; this sheet is what that falls back to where no such chooser
	 * exists, and the reason `folders.rs` exists at all.
	 */
	let listing = $state<FolderListing | null>(null);
	/** Why the last listing failed; a folder that cannot be read, usually. */
	let error = $state("");
	let loading = $state(false);

	/** The folder being listed, or `null` for home. */
	async function browse(path: string | null) {
		loading = true;
		error = "";
		try {
			const result = await commands.listFolders(path);
			if (result.status === "error") {
				error = result.error;
				return;
			}
			listing = result.data;
		} catch (cause) {
			error = messageOf(cause);
		} finally {
			loading = false;
		}
	}

	/** Open the folder on screen; a refusal keeps the sheet up and says why. */
	async function openHere() {
		if (!listing) return;
		if (await openLedgerPath(listing.path)) close();
	}

	/** Forget where the picker was, so the next opening starts at home again. */
	function forget() {
		listing = null;
		error = "";
	}

	/** Close the sheet from inside it, forgetting the folder on the way out. */
	function close() {
		forget();
		closeFolderPicker();
	}

	$effect(() => {
		// Opening the sheet starts at home, once per opening.
		if (ledger.pickerOpen && !listing) browse(null);
	});
</script>

<Sheet.Root
	open={ledger.pickerOpen}
	onOpenChange={(open) => {
		ledger.pickerOpen = open;
		// The overlay, Escape and the sheet's own close button come through
		// here, next to the in-sheet Cancel below.
		if (!open) forget();
	}}
>
	<Sheet.Content side="bottom" class="p-0">
		<!--
			The sheet's own height is auto; this fixed-height column is what gives
			the folder list something to scroll inside.
		-->
		<div class="mx-auto flex h-[70dvh] w-full max-w-xl flex-col">
			<Sheet.Header class="p-4 pb-2">
				<Sheet.Title>Open ledger folder</Sheet.Title>
				<Sheet.Description>
					Pick the folder holding a ledger's root journal — main.bean, ledger.bean, and
					the rest of the loader's names. Folders that have one are marked.
				</Sheet.Description>
			</Sheet.Header>

			<div class="flex min-h-0 flex-1 flex-col gap-2 px-4 pb-4">
				<div class="flex items-center gap-1">
					<Button
						size="icon-sm"
						variant="ghost"
						aria-label="Parent folder"
						disabled={!listing?.parent || loading}
						onclick={() => listing?.parent && browse(listing.parent)}
					>
						<HugeiconsIcon icon={ArrowUp01Icon} strokeWidth={2} />
					</Button>
					<span class="min-w-0 truncate font-mono text-xs" title={listing?.path ?? ""}>
						{listing?.path ?? "…"}
					</span>
				</div>

				{#if error}
					<Alert variant="destructive">{error}</Alert>
				{/if}

				<div class="border-border min-h-0 flex-1 overflow-y-auto rounded-md border">
					{#if listing?.folders.length}
						<ul class="divide-border divide-y">
							{#each listing.folders as folder (folder.path)}
								<li>
									<!--
										A whole row is the target, tall enough for a thumb;
										the journal-bearing folders are the ones worth
										walking into and are named as such.
									-->
									<button
										type="button"
										class="hover:bg-muted focus-visible:bg-muted flex w-full items-center gap-2 px-3 py-2.5 text-left outline-none"
										onclick={() => browse(folder.path)}
									>
										<HugeiconsIcon
											icon={folder.ledger ? FolderCheckIcon : Folder01Icon}
											strokeWidth={2}
											class={cn(
												"size-4 shrink-0",
												folder.ledger ? "text-foreground" : "text-muted-foreground"
											)}
										/>
										<span class="min-w-0 flex-1 truncate text-sm">{folder.name}</span>
										{#if folder.ledger}
											<span class="text-muted-foreground text-xs">ledger</span>
										{/if}
									</button>
								</li>
							{/each}
						</ul>
					{:else if listing}
						<p class="text-muted-foreground p-3 text-xs">No folders inside.</p>
					{/if}
				</div>

				<div class="flex flex-wrap items-center gap-2">
					<Button
						size="sm"
						disabled={!listing?.ledger || ledger.opening}
						onclick={openHere}
					>
						{ledger.opening ? "Opening…" : "Open this folder"}
					</Button>
					<Button size="sm" variant="ghost" onclick={close}>Cancel</Button>
					{#if listing && !listing.ledger}
						<span class="text-muted-foreground text-xs">No root journal here.</span>
					{/if}
				</div>

				{#if ledger.error}
					<Alert variant="destructive">{ledger.error}</Alert>
				{/if}
			</div>
		</div>
	</Sheet.Content>
</Sheet.Root>
