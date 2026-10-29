// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later
import { commands, type BranchInfo, type CommitInfo } from "../../bindings";
import { changedPaths, repoRelative } from "$lib/components/git/git.js";
import { messageOf } from "$lib/errors.js";
import type { PageLoad } from "./$types";

/** The log's first page: newest 50 commits, which is what the screen opens on. */
const FIRST_PAGE = 50;

/** The repository-relative and absolute path of one journal file. */
type JournalPath = { relative: string; absolute: string };

/** What the branches screen reads before it draws anything. */
type BranchesData = {
	/** Whether the ledger's directory is inside a repository. */
	repo: boolean;
	/** `repo` is false because no ledger is open, not because git is missing. */
	noLedger: boolean;
	root: string | null;
	branches: BranchInfo[];
	commits: CommitInfo[];
	journal: JournalPath[];
	/** The ledger's uncommitted files, from `ledger_diff`'s own blocks. */
	tree: string[];
	limit: number;
	/** A failure of the whole read (the repository state itself). */
	error: string;
	branchesError: string;
	logError: string;
	treeError: string;
};

/**
 * Everything the branches screen needs: whether there is a repository and a
 * ledger, the branches, the newest page of the log, the ledger's changed files
 * and the journal's files.
 *
 * The journal files are read for one reason: `git_resolve` stages a merged text
 * through the ledger's own writer, which only knows the journal's files, and
 * names the file it cannot write when asked for another. Matching a conflict
 * path to its absolute journal path makes the path independent of where the
 * repository root sits relative to the ledger directory — and lets the resolver
 * say before a click that a path is outside the journal.
 *
 * Reads fail one by one: a broken log does not blank the branch list, and a
 * repository with nothing committed is an empty history, not an error.
 */
export const load: PageLoad = async (): Promise<BranchesData> => {
	const empty: BranchesData = {
		repo: false,
		noLedger: false,
		root: null,
		branches: [],
		commits: [],
		journal: [],
		tree: [],
		limit: FIRST_PAGE,
		error: "",
		branchesError: "",
		logError: "",
		treeError: "",
	};
	try {
		const state = await commands.gitState();
		if (!state.repo) {
			// `repo: false` also means "no ledger open": the two states need
			// different words and a different first step.
			try {
				const summary = await commands.ledgerSummary();
				return { ...empty, noLedger: summary === null };
			} catch (cause) {
				console.error("ledgerSummary failed", cause);
				return { ...empty, error: messageOf(cause) };
			}
		}
		const root = state.root;
		const data: BranchesData = { ...empty, repo: true, root };
		await Promise.all([
			(async () => {
				try {
					const answer = await commands.gitBranches();
					if (answer.status === "error") data.branchesError = answer.error;
					else data.branches = answer.data;
				} catch (cause) {
					console.error("gitBranches failed", cause);
					data.branchesError = messageOf(cause);
				}
			})(),
			(async () => {
				try {
					const answer = await commands.gitLog(FIRST_PAGE, null);
					if (answer.status === "error") data.logError = answer.error;
					else data.commits = answer.data;
				} catch (cause) {
					console.error("gitLog failed", cause);
					data.logError = messageOf(cause);
				}
			})(),
			(async () => {
				try {
					const answer = await commands.ledgerDiff();
					if (answer.status === "error") data.treeError = answer.error;
					else data.tree = changedPaths(answer.data);
				} catch (cause) {
					console.error("ledgerDiff failed", cause);
					data.treeError = messageOf(cause);
				}
			})(),
			(async () => {
				try {
					data.journal = (await commands.journalFiles()).map((file) => ({
						relative: repoRelative(root, file.path),
						absolute: file.path,
					}));
				} catch (cause) {
					// The resolver still works from the merge's own paths; it just
					// cannot pre-check which ones are journal files.
					console.error("journalFiles failed", cause);
				}
			})(),
		]);
		return data;
	} catch (cause) {
		console.error("gitState failed", cause);
		return { ...empty, error: messageOf(cause) };
	}
};
