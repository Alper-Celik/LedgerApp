<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { commands } from "../bindings";
	import { Button } from "$lib/components/ui/button/index.js";
	import { Input } from "$lib/components/ui/input/index.js";

	let name = $state("");
	let greetMsg = $state("");

	async function greet(event: Event) {
		event.preventDefault();
		greetMsg = await commands.greet(name);
	}
</script>

<div class="flex flex-col gap-4 p-4">
	<h1 class="text-2xl font-semibold tracking-tight">Ledger</h1>

	<form class="flex max-w-sm items-center gap-2" onsubmit={greet}>
		<Input id="greet-input" placeholder="Enter a name..." bind:value={name} />
		<Button type="submit">Greet</Button>
	</form>
	{#if greetMsg}
		<p class="text-muted-foreground text-sm">{greetMsg}</p>
	{/if}
</div>
