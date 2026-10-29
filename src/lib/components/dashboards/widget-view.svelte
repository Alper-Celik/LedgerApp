<!--
SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>

SPDX-License-Identifier: AGPL-3.0-or-later
-->
<script lang="ts">
	import type { ChartDatum, ChartSeries } from "$lib/charts/types.js";
	import { definitionFor } from "$lib/dashboard/index.js";
	import { Alert } from "$lib/components/ui/alert/index.js";
	import BalanceTable from "./balance-table.svelte";
	import WidgetCard from "./widget-card.svelte";
	import WidgetChart from "./widget-chart.svelte";
	import WidgetEmpty from "./widget-empty.svelte";
	import {
		balanceRows,
		budgetSeries,
		cashFlowSeries,
		categoryShare,
		isWidgetKind,
		netWorthSeries,
		spendingSeries,
		type BalanceRow,
		type DashboardData,
		type PlacedWidget,
	} from "./data.js";

	/**
	 * One widget, drawn by its kind.
	 *
	 * Every kind reads the data the route fetched once; the widget never asks a
	 * command of its own, so a page of six widgets costs one read per report.
	 * A missing source or a kind with no implementation is answered inside this
	 * card, never by failing the page around it.
	 */
	let { widget, data }: { widget: PlacedWidget; data: DashboardData } = $props();

	const definition = $derived(isWidgetKind(widget.kind) ? definitionFor(widget.kind) : null);

	/**
	 * A source's error, or `""` when it answered. The per-kind derivations below
	 * still run on the empty fallback, so the template can branch on the error
	 * without touching the union in markup.
	 */
	const accountsError = $derived(data.accounts.ok ? "" : data.accounts.error);
	const netWorthError = $derived(data.netWorth.ok ? "" : data.netWorth.error);
	const cashflowError = $derived(data.cashflow.ok ? "" : data.cashflow.error);
	const categoriesError = $derived(data.categories.ok ? "" : data.categories.error);
	const budgetError = $derived(data.budget.ok ? "" : data.budget.error);

	const netWorth: ChartSeries[] = $derived(
		data.netWorth.ok ? netWorthSeries(data.netWorth.data, widget.config) : []
	);
	const cashFlow: ChartSeries[] = $derived(
		data.cashflow.ok ? cashFlowSeries(data.cashflow.data, widget.config) : []
	);
	const spending: ChartSeries[] = $derived(
		data.categories.ok ? spendingSeries(data.categories.data, widget.config) : []
	);
	const share: ChartDatum[] = $derived(
		data.categories.ok ? categoryShare(data.categories.data, widget.config) : []
	);
	const balances: BalanceRow[] = $derived(
		data.accounts.ok ? balanceRows(data.accounts.data, widget.config) : []
	);
	const budgetPoints: ChartSeries[] = $derived(
		data.budget.ok ? budgetSeries(data.budget.data, widget.config) : []
	);
	const hasBudgetMonths = $derived(data.budget.ok && data.budget.data.months.length > 0);
</script>

{#if !definition}
	<WidgetCard title="Unknown widget" hint={widget.kind}>
		<WidgetEmpty
			title="This build does not know this widget kind"
			detail={`“${widget.kind}” was saved by another version of the app. Edit the dashboard to remove it.`}
		/>
	</WidgetCard>
{:else if widget.kind === "net-worth"}
	<WidgetCard title={definition.title} hint={definition.description}>
		{#if netWorthError}
			<Alert variant="destructive">{netWorthError}</Alert>
		{:else if netWorth.length === 0}
			<WidgetEmpty
				title="No net worth to draw in this window"
				detail="The net-worth report answered no months for this dashboard's window, or none in the configured commodity."
			/>
		{:else}
			<WidgetChart name="area" series={netWorth} />
		{/if}
	</WidgetCard>
{:else if widget.kind === "lifestyle-inflation"}
	<WidgetCard title={definition.title} hint={definition.description}>
		<WidgetEmpty
			title="Not drawn from a dashboard source yet"
			detail="This widget needs the lifestyle report's own index series, which dashboards are not given yet."
		/>
	</WidgetCard>
{:else if widget.kind === "cash-flow"}
	<WidgetCard title={definition.title} hint={definition.description}>
		{#if cashflowError}
			<Alert variant="destructive">{cashflowError}</Alert>
		{:else if cashFlow.length === 0}
			<WidgetEmpty
				title="No income or expense entries in this window"
				detail="Cash flow is read from the ledger's Income and Expenses postings, month by month."
			/>
		{:else}
			<WidgetChart name="bar" series={cashFlow} />
		{/if}
	</WidgetCard>
{:else if widget.kind === "spending-by-month"}
	<WidgetCard title={definition.title} hint={definition.description}>
		{#if categoriesError}
			<Alert variant="destructive">{categoriesError}</Alert>
		{:else if spending.length === 0}
			<WidgetEmpty
				title="No spending in this window"
				detail="Every month the report covers gets a bar, and months without Expenses postings stay at zero."
			/>
		{:else}
			<WidgetChart name="bar" series={spending} />
		{/if}
	</WidgetCard>
{:else if widget.kind === "budget-vs-actual"}
	<WidgetCard title={definition.title} hint={definition.description}>
		{#if budgetError}
			<Alert variant="destructive">{budgetError}</Alert>
		{:else if !hasBudgetMonths}
			<WidgetEmpty
				title="No envelope budget yet"
				detail="This ledger has no budgeted months — the envelope plugin writes them on load, and this widget reads the range it reports."
			/>
		{:else}
			<WidgetChart name="bar" series={budgetPoints} />
		{/if}
	</WidgetCard>
{:else if widget.kind === "category-share"}
	<WidgetCard title={definition.title} hint={definition.description}>
		{#if categoriesError}
			<Alert variant="destructive">{categoriesError}</Alert>
		{:else if share.length === 0}
			<WidgetEmpty
				title="Nothing to divide up"
				detail="No expense branch in the last few months has a positive total to show as a share."
			/>
		{:else}
			<WidgetChart name="donut" data={share} />
		{/if}
	</WidgetCard>
{:else if widget.kind === "balances-table"}
	<WidgetCard title={definition.title} hint={definition.description}>
		{#if accountsError}
			<Alert variant="destructive">{accountsError}</Alert>
		{:else if balances.length === 0}
			<WidgetEmpty
				title="No balances to show"
				detail="Every account at this depth holds zero, or the account filter matches none of them."
			/>
		{:else}
			<BalanceTable rows={balances} />
		{/if}
	</WidgetCard>
{:else}
	<!--
		A kind the registry lists but this component has no body for. It is a
		programming mistake rather than a data problem, so it says so instead of
		drawing a blank card.
	-->
	<WidgetCard title={definition.title} hint={definition.description}>
		<WidgetEmpty title="This widget has no renderer yet" detail={definition.kind} />
	</WidgetCard>
{/if}
