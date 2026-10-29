<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import { onMount } from "svelte";
	import { Badge } from "$lib/components/ui/badge/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { Input } from "$lib/components/ui/input/index.js";
	import { Textarea } from "$lib/components/ui/textarea/index.js";
	import * as ToggleGroup from "$lib/components/ui/toggle-group/index.js";
	import {
		fromCss,
		fromJson,
		sanitiseValue,
		toHexColor,
		type ThemeParseError,
	} from "$lib/theme/parse.js";
	import { apply, load, reset, save, theme } from "$lib/theme/store.svelte.js";
	import {
		TOKEN_GROUPS,
		emptyTheme,
		isGlobalToken,
		type Theme,
		type ThemeMode,
		type ThemeTokens,
		type TokenSpec,
	} from "$lib/theme/types.js";

	let mode = $state<ThemeMode>("light");
	let name = $state("Custom");
	/** Only what the user has changed: anything absent still shows `app.css`. */
	let light = $state<ThemeTokens>({});
	let dark = $state<ThemeTokens>({});
	let importText = $state("");
	let importErrors = $state<ThemeParseError[]>([]);
	let importMessage = $state("");

	const modeGroups = TOKEN_GROUPS.filter((group) => group.modeScoped);
	const shapeGroup = TOKEN_GROUPS.find((group) => !group.modeScoped);

	onMount(() => {
		load();
		const seed = theme.custom ?? theme.active ?? emptyTheme();
		name = seed.name;
		light = { ...seed.light };
		dark = { ...seed.dark };
	});

	/** The map a token is edited in: the mode block, or `:root` for shared tokens. */
	function mapFor(token: string): ThemeTokens {
		if (isGlobalToken(token)) return light;
		return mode === "dark" ? dark : light;
	}

	/** The value shown: what the user typed, else what the spec sheet declares. */
	function valueOf(token: string): string {
		const own = mapFor(token)[token];
		if (own !== undefined) return own;
		const defaults = theme.defaults;
		if (isGlobalToken(token)) return defaults.light[token] ?? "";
		if (mode === "dark") return defaults.dark[token] ?? defaults.light[token] ?? "";
		return defaults.light[token] ?? "";
	}

	/** Why a typed value cannot be applied, or `null` when it is fine. */
	function problemOf(token: string): string | null {
		const value = valueOf(token);
		if (!value.trim()) return null;
		const result = sanitiseValue(token, value);
		return result.ok ? null : result.error;
	}

	/** Only sanitised entries reach the stylesheet; invalid text stays in the field. */
	function clean(tokens: ThemeTokens): ThemeTokens {
		const cleaned: ThemeTokens = {};
		for (const [token, value] of Object.entries(tokens)) {
			const result = sanitiseValue(token, value);
			if (result.ok) cleaned[token] = result.value;
		}
		return cleaned;
	}

	function draft(): Theme {
		return {
			id: theme.custom?.id ?? "custom",
			name,
			light: clean(light),
			dark: clean(dark),
		};
	}

	function commit() {
		const next = draft();
		if (Object.keys(next.light).length === 0 && Object.keys(next.dark).length === 0) {
			reset();
			return;
		}
		apply(next);
		save(next);
	}

	function setToken(token: string, raw: string) {
		const map = mapFor(token);
		if (raw.trim()) map[token] = raw;
		else delete map[token];
		commit();
	}

	function hexSeed(token: string): string {
		return toHexColor(valueOf(token)) ?? "#000000";
	}

	function onImport() {
		const text = importText.trim();
		importErrors = [];
		importMessage = "";
		if (!text) return;
		const result = text.startsWith("{") ? fromJson(text) : fromCss(text);
		importErrors = result.errors;
		const count = Object.keys(result.theme.light).length + Object.keys(result.theme.dark).length;
		if (count === 0) {
			importMessage = "Nothing to apply — the paste holds no valid token.";
			return;
		}
		name = result.theme.name;
		light = { ...result.theme.light };
		dark = { ...result.theme.dark };
		const next = draft();
		apply(next);
		save(next);
		importMessage = `Applied ${count} token${count === 1 ? "" : "s"}${
			result.errors.length ? `; ${result.errors.length} skipped` : ""
		}.`;
	}

	function onReset() {
		reset();
		name = "Custom";
		light = {};
		dark = {};
		importText = "";
		importErrors = [];
		importMessage = "";
	}
</script>

<div class="flex flex-col gap-4">
	<div class="flex flex-wrap items-center justify-between gap-2">
		<ToggleGroup.Root
			type="single"
			value={mode}
			onValueChange={(next) => {
				if (next) mode = next as ThemeMode;
			}}
			aria-label="Edit which mode"
		>
			<ToggleGroup.Item value="light">Light</ToggleGroup.Item>
			<ToggleGroup.Item value="dark">Dark</ToggleGroup.Item>
		</ToggleGroup.Root>
		<span class="text-muted-foreground min-w-0 truncate text-[0.6875rem]">
			{theme.active ? `overriding ${theme.active.name}` : "shipped theme, no overrides"}
		</span>
	</div>

	<!--
		The preview is real components over the live tokens, so it needs no
		second stylesheet: the `.dark` wrapper re-declares the dark block for
		the subtree when the dark tab is selected.
	-->
	<div class={mode === "dark" ? "dark" : ""}>
		<div class="bg-background text-foreground border-border flex flex-col gap-2 rounded-lg border p-3">
			<div class="flex flex-wrap items-center gap-1.5">
				<Button size="sm">Post</Button>
				<Button size="sm" variant="secondary">Cancel</Button>
				<Badge>reconciled</Badge>
				<Badge variant="destructive">overdraft</Badge>
			</div>
			<div
				class="border-border bg-card text-card-foreground flex flex-col gap-1.5 rounded-[var(--radius)] border p-2"
			>
				<span class="text-muted-foreground text-[0.625rem]">
					card, muted foreground, input, radius
				</span>
				<Input class="h-7 text-xs" value="1,240.00 EUR" readonly aria-label="Preview amount" />
			</div>
			<div class="flex gap-1" aria-hidden="true">
				{#each [1, 2, 3, 4, 5] as series (series)}
					<span
						class="h-5 flex-1 rounded-sm"
						style="background: var(--chart-{series})"
					></span>
				{/each}
			</div>
			<div class="flex flex-wrap items-center gap-2 text-[0.625rem]">
				<span style="color: var(--added)">+ added</span>
				<span style="color: var(--removed)">− removed</span>
				<span class="border-border border px-1 py-0.5" style="background: var(--sidebar)">
					sidebar
				</span>
			</div>
		</div>
	</div>

	{#each modeGroups as group (group.id)}
		<section class="flex flex-col gap-1.5">
			<div class="flex flex-col gap-0.5">
				<h3 class="text-xs font-medium">{group.label}</h3>
				<p class="text-muted-foreground text-[0.625rem]">{group.hint}</p>
			</div>
			<div class="flex flex-col gap-1">
				{#each group.tokens as spec (spec.token)}
					{@render tokenRow(spec)}
				{/each}
			</div>
		</section>
	{/each}

	{#if shapeGroup}
		<section class="flex flex-col gap-1.5">
			<div class="flex flex-col gap-0.5">
				<h3 class="text-xs font-medium">{shapeGroup.label}</h3>
				<p class="text-muted-foreground text-[0.625rem]">{shapeGroup.hint}</p>
			</div>
			<div class="flex flex-col gap-1">
				{#each shapeGroup.tokens as spec (spec.token)}
					{@render tokenRow(spec)}
				{/each}
			</div>
		</section>
	{/if}

	<section class="border-border flex flex-col gap-2 rounded-md border p-2">
		<div class="flex flex-col gap-0.5">
			<h3 class="text-xs font-medium">Import</h3>
			<p class="text-muted-foreground text-[0.625rem]">
				Paste a theme builder export: CSS with a <code class="font-mono">:root</code> block
				(and optionally <code class="font-mono">.dark</code>), or JSON shaped as
				<code class="font-mono">&#123; light, dark &#125;</code>, as flat
				<code class="font-mono">--token: value</code> pairs, or as the registry's
				<code class="font-mono">cssVars</code> object. Every value is checked before use; a
				rejected token is listed and skipped.
			</p>
		</div>
		<Textarea
			class="min-h-24 font-mono text-[0.6875rem]"
			bind:value={importText}
			placeholder={":root {\n  --background: oklch(1 0 0);\n}"}
			aria-label="Theme CSS or JSON"
		/>
		<div class="flex flex-wrap items-center gap-2">
			<Button size="sm" onclick={onImport} disabled={!importText.trim()}>
				Import and apply
			</Button>
			<Button
				size="sm"
				variant="ghost"
				onclick={() => {
					importText = "";
					importErrors = [];
					importMessage = "";
				}}
			>
				Clear
			</Button>
		</div>
		{#if importMessage}
			<p class="text-muted-foreground text-[0.6875rem]">{importMessage}</p>
		{/if}
		{#if importErrors.length > 0}
			<ul class="text-destructive flex flex-col gap-1 text-[0.625rem]">
				{#each importErrors as error, index (index)}
					<li class="break-words">
						<code class="font-mono">{error.token}</code>
						{error.message}
					</li>
				{/each}
			</ul>
		{/if}
	</section>

	<div class="flex flex-wrap items-center gap-2">
		<Button
			size="sm"
			variant="secondary"
			onclick={onReset}
			disabled={!theme.active && Object.keys(light).length === 0 && Object.keys(dark).length === 0}
		>
			Reset to shipped theme
		</Button>
		<span class="text-muted-foreground min-w-0 flex-1 text-[0.625rem]">
			Changes apply as you type and are saved for the next launch.
		</span>
	</div>
</div>

{#snippet tokenRow(spec: TokenSpec)}
	<div class="flex items-center gap-2">
		{#if spec.kind === "color"}
			<input
				type="color"
				class="border-border bg-muted size-6 shrink-0 cursor-pointer rounded border p-0.5"
				value={hexSeed(spec.token)}
				aria-label={`${spec.label} colour picker`}
				oninput={(event) => setToken(spec.token, event.currentTarget.value)}
			/>
		{:else}
			<span class="size-6 shrink-0" aria-hidden="true"></span>
		{/if}
		<span
			class="text-muted-foreground w-[6.75rem] shrink-0 truncate font-mono text-[0.625rem]"
			title={spec.token}>{spec.token}</span
		>
		<Input
			class="h-7 min-w-0 flex-1 font-mono text-[0.6875rem]"
			value={valueOf(spec.token)}
			aria-label={spec.label}
			aria-invalid={problemOf(spec.token) !== null}
			title={problemOf(spec.token) ?? spec.token}
			placeholder={spec.kind === "font" ? "'Inter Variable', sans-serif" : "—"}
			oninput={(event) => setToken(spec.token, event.currentTarget.value)}
		/>
	</div>
{/snippet}
