// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { commands, type PriceHistory } from "../../../bindings";
import { messageOf } from "$lib/errors.js";
import type { PageLoad } from "./$types";

/**
 * One commodity's price history as the screen opens: both sources, every
 * currency, every day the journal states. The controls narrow the answer on
 * the page itself — each change asks the backend again — so this load only has
 * to get the commodity's identity and its first, widest answer.
 *
 * A backend failure comes back as this page's own `error` rather than a throw:
 * a ledger with no history is a state the screen explains, while SvelteKit's
 * error page would read as the app being broken.
 */
export const load: PageLoad = async ({ params }) => {
	const commodity = params.commodity;
	try {
		const result = await commands.priceHistory(commodity, null, "both", null, null, null);
		if (result.status === "error") {
			return { commodity, history: null as PriceHistory | null, error: result.error };
		}
		return { commodity, history: result.data, error: "" };
	} catch (cause) {
		console.error("price history load failed", cause);
		return { commodity, history: null as PriceHistory | null, error: messageOf(cause) };
	}
};
