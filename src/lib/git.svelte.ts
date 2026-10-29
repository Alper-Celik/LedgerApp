// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

import { commands } from "../bindings";
import { messageOf } from "./errors.js";

/**
 * What the backend's `git_state` answers: whether the ledger's directory is a
 * repository, where that repository starts, and the identity commits would be
 * attributed to. Written out here because the generated bindings type arrives
 * with the command itself.
 */
type GitState = {
	repo: boolean;
	root: string | null;
	user_name: string | null;
	user_email: string | null;
};

/**
 * The ledger's git state, in one place. The diff view, the commit view and the
 * sidebar all have to answer "is this ledger under git at all" before they can
 * offer anything — a diff needs a repository to diff against, the sidebar's
 * uncommitted marks need one too — so they read this store instead of each
 * asking the backend on their own.
 *
 * `repo` starts false: nothing is known before the first refresh, and no view
 * should offer a commit against a repository nobody has confirmed exists.
 */
export const git = $state<GitState>({
	repo: false,
	root: null,
	user_name: null,
	user_email: null,
});

/**
 * Read the state again. Called on mount by whoever shows it and after every
 * command that changes it (initialising, writing the identity), so the store
 * never drifts from the repository.
 */
export async function refreshGit() {
	try {
		const state = await commands.gitState();
		git.repo = state.repo;
		git.root = state.root;
		git.user_name = state.user_name;
		git.user_email = state.user_email;
	} catch {
		// No backend — a plain browser during development. Nothing to report.
	}
}

/**
 * Create a repository in the ledger's directory. The command is idempotent, so
 * asking twice is harmless; the state is refreshed on success so every view
 * that gates on `git.repo` starts working at once.
 *
 * Returns the command's own refusal, or the IPC failure, as text to show; `null`
 * means it worked. Nothing is thrown into rendering.
 */
export async function initialise(): Promise<string | null> {
	try {
		const result = await commands.gitInit();
		if (result.status === "error") return result.error;
		await refreshGit();
		return null;
	} catch (cause) {
		return messageOf(cause);
	}
}

/**
 * Write `user_name`/`user_email` into the repository's config. Same contract as
 * `initialise`: the refusal comes back as text, success refreshes the store.
 */
export async function saveIdentity(name: string, email: string): Promise<string | null> {
	try {
		const result = await commands.gitSetIdentity(name, email);
		if (result.status === "error") return result.error;
		await refreshGit();
		return null;
	} catch (cause) {
		return messageOf(cause);
	}
}
