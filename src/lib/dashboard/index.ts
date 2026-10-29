// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
export { GRID_CLASS, WIDGET_SPANS, spanClasses } from "./layout.js";
export {
	chartLoaders,
	definitionFor,
	widgetDefinitions,
	type ChartComponent,
	type ChartModule,
	type ChartName,
	type WidgetDefinition,
} from "./registry.js";
export { isWidgetSpan } from "./types.js";
export type { JsonValue, WidgetConfig, WidgetKind, WidgetSpan } from "./types.js";
