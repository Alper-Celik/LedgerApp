<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import TransactionBrowser from "$lib/components/transaction-browser.svelte";
	import type { PageProps } from "./$types";

	let { data }: PageProps = $props();
</script>

<TransactionBrowser account={data.account} transactions={data.transactions} error={data.error} />
