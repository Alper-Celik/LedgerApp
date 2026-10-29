<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import DashboardEditor from "$lib/components/dashboards/dashboard-editor.svelte";
	import type { PageProps } from "./$types";

	/**
	 * The editor route.
	 *
	 * The load answers the saved dashboard (or `null` for a new one); the draft
	 * lives in `DashboardEditor`, keyed by id so a move to another dashboard's
	 * editor starts from that dashboard rather than carrying unsaved edits over.
	 */
	let { data }: PageProps = $props();
</script>

{#key data.id}
	<DashboardEditor id={data.id} dashboard={data.dashboard} loadError={data.error} />
{/key}
