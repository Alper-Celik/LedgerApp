<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->

<script lang="ts">
 import * as Sidebar from "$lib/components/ui/sidebar/index.js";
 import AppSidebar from "$lib/components/app-sidebar.svelte";
 import "../app.css";
 import { ModeWatcher } from "mode-watcher";
 
 let { children } = $props();
</script>
 
<Sidebar.Provider>
 <ModeWatcher />
 <AppSidebar />
 <Sidebar.Inset>
  <header class="flex h-12 items-center gap-2 px-4">
   <Sidebar.Trigger />
  </header>
  {@render children?.()}
 </Sidebar.Inset>
</Sidebar.Provider>
