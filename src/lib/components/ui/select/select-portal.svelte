<!--
SPDX-FileCopyrightText: 2026 shadcn-svelte contributors <https://github.com/huntabyte/shadcn-svelte>

SPDX-License-Identifier: MIT
-->
<script lang="ts">
	import { Select as SelectPrimitive } from "bits-ui";

	let { ...restProps }: SelectPrimitive.PortalProps = $props();
</script>

<SelectPrimitive.Portal {...restProps} />
