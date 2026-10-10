<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { page } from "$app/state";

	// TODO(alper): load the account's ledger entries via a typed command here.
	const account = page.params.id;
</script>

<div class="flex flex-col gap-4 p-4">
	<h1 class="text-2xl font-semibold tracking-tight">{account}</h1>
</div>
