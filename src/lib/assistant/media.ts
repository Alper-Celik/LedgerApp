// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * Files picked for one turn, handed to the backend as data parts.
 *
 * The picker accepts images, PDFs and CSVs. An image or a CSV goes as the
 * browser read it; a PDF that sits in the ledger is rasterised by the backend
 * and sent page by page as PNG images, because that is what a vision model
 * reads — see [`mediaPartsOf`]. The endpoint's own model list is the only
 * voice that says whether the selected model takes anything visual, so an
 * attachment is only sent when `aiModels` marked the model vision-capable;
 * otherwise the UI says so instead of shipping bytes the endpoint cannot read.
 *
 * Limits are deliberately the frontend's: the browser already has the bytes,
 * and a 10 MB file should be refused before it is encoded and passed over IPC.
 */
import { commands } from "../../bindings";
import { messageOf } from "../errors.js";
import type { MediaPart, ModelInfo } from "./api.js";

/** The most files one message may carry. */
export const MAX_ATTACHMENTS = 3;

/** The most one file may weigh. */
export const MAX_ATTACHMENT_BYTES = 10 * 1024 * 1024;

/**
 * The dpi pages are rasterised at: A4 lands at 1240 × 1754, which is what the
 * vision models this panel talks to read best without shipping a huge image.
 */
export const PDF_DPI = 150;

/** The most pages one PDF contributes; the backend's own pixel budget caps the rest. */
export const PDF_MAX_PAGES = 20;

/** What the picker offers. Extensions are repeated for platforms with poor MIME mapping. */
export const ATTACHMENT_ACCEPT = "image/*,application/pdf,text/csv,.pdf,.csv";

export type Attachment = {
	/** The file's name, for the chip. */
	name: string;
	/** Its media type, guessed from the name when the browser does not give one. */
	mime: string;
	/** Its size in bytes, for the chip. */
	size: number;
	/** The complete `data:` URL — what `MediaPart.data_base64` accepts whole. */
	dataUrl: string;
	/**
	 * Where a PDF sits relative to the ledger, as `attachment_pdf_pages` takes
	 * it — the command confines the path to the ledger's own directory. The
	 * picked file's name is the guess, editable in the chip because a picker
	 * does not say which folder the file came from; empty means "send the
	 * browser's bytes instead", which is the only route a PDF outside the
	 * ledger has. `null` for an attachment that is not a PDF.
	 */
	path: string | null;
};

const MIME_BY_EXTENSION: Record<string, string> = {
	pdf: "application/pdf",
	csv: "text/csv",
	png: "image/png",
	jpg: "image/jpeg",
	jpeg: "image/jpeg",
	gif: "image/gif",
	webp: "image/webp",
	bmp: "image/bmp",
	svg: "image/svg+xml",
};

/** The file's type, the browser's first and the extension's as a fallback. */
export function mimeOf(file: File): string {
	const extension = file.name.split(".").pop()?.toLowerCase() ?? "";
	// Some platforms report CSV and PDF as generic binary or spreadsheet types;
	// the extension is authoritative for the two formats this picker names.
	if (extension === "csv") return "text/csv";
	if (extension === "pdf") return "application/pdf";
	if (file.type) return file.type;
	return MIME_BY_EXTENSION[extension] ?? "";
}

/** Whether the type is one the picker offers. */
export function isSupported(mime: string): boolean {
	return mime.startsWith("image/") || mime === "application/pdf" || mime === "text/csv";
}

/** Whether the endpoint's model list marks `model` as taking images. */
export function visionOf(models: ModelInfo[], model: string): "yes" | "no" | "unknown" {
	if (!model) return "unknown";
	const info = models.find((candidate) => candidate.id === model);
	if (!info || info.vision === null || info.vision === undefined) return "unknown";
	return info.vision ? "yes" : "no";
}

/** Bytes as the chip says them. */
export function formatBytes(bytes: number): string {
	if (bytes < 1024) return `${bytes} B`;
	if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
	return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/**
 * Read the chosen files, refusing what cannot be attached.
 *
 * The refusals are this module's own (size, count, type, an unreadable file);
 * backend refusals never pass through here.
 */
export async function readFiles(
	files: File[],
	current: Attachment[]
): Promise<{ attachments: Attachment[]; errors: string[] }> {
	const attachments: Attachment[] = [];
	const errors: string[] = [];

	for (const file of files) {
		if (current.length + attachments.length >= MAX_ATTACHMENTS) {
			errors.push(`Only ${MAX_ATTACHMENTS} files can be attached to one message.`);
			break;
		}
		if (file.size > MAX_ATTACHMENT_BYTES) {
			errors.push(
				`"${file.name}" is ${formatBytes(file.size)}; attachments are capped at ${formatBytes(MAX_ATTACHMENT_BYTES)}.`
			);
			continue;
		}
		const mime = mimeOf(file);
		if (!isSupported(mime)) {
			errors.push(`"${file.name}" is not an image, PDF, or CSV.`);
			continue;
		}
		try {
			attachments.push({
				name: file.name,
				mime,
				size: file.size,
				dataUrl: await readAsDataUrl(file),
				// A picker hands over bytes, never a folder: the name beside the
				// journal is the guess, and the chip lets it be corrected.
				path: mime === "application/pdf" ? file.name : null,
			});
		} catch (cause) {
			errors.push(cause instanceof Error ? cause.message : String(cause));
		}
	}

	return { attachments, errors };
}

function readAsDataUrl(file: File): Promise<string> {
	const { promise, resolve, reject } = Promise.withResolvers<string>();
	const reader = new FileReader();
	reader.onload = () => {
		if (typeof reader.result === "string") resolve(reader.result);
		else reject(new Error(`"${file.name}" could not be read.`));
	};
	reader.onerror = () => {
		reject(new Error(reader.error?.message ?? `"${file.name}" could not be read.`));
	};
	reader.readAsDataURL(file);
	return promise;
}

/** A PNG's bytes as the `data:` URL a media part carries. */
function pngDataUrl(bytes: readonly number[]): string {
	let binary = "";
	// `String.fromCharCode` takes its arguments on the stack, so the byte array
	// is walked in chunks rather than spread whole.
	const chunk = 0x8000;
	for (let at = 0; at < bytes.length; at += chunk) {
		binary += String.fromCharCode(...bytes.slice(at, at + chunk));
	}
	return `data:image/png;base64,${btoa(binary)}`;
}

/**
 * The media parts one attachment contributes, and the refusal when it
 * contributed none.
 *
 * A PDF that names its place in the ledger is rasterised by the backend
 * (`attachment_pdf_pages` at [`PDF_DPI`], at most [`PDF_MAX_PAGES`] pages) and
 * sent as one PNG image part per page: the vision model reads pages, not a PDF
 * container. An attachment with no ledger path — every image and CSV, and a
 * PDF whose path the user cleared — goes as the browser already read it. A PDF
 * the rasteriser refuses is reported, not replaced by its raw bytes: a ledger
 * file that cannot be rendered is a fact the turn should carry.
 */
export async function mediaPartsOf(
	attachment: Attachment
): Promise<{ parts: MediaPart[]; error: string }> {
	const path = attachment.path?.trim() ?? "";
	if (attachment.mime !== "application/pdf" || path === "") {
		return {
			parts: [{ mime: attachment.mime, data_base64: attachment.dataUrl, detail: null }],
			error: "",
		};
	}
	try {
		const answer = await commands.attachmentPdfPages(path, PDF_DPI, PDF_MAX_PAGES);
		if (answer.status === "error") throw new Error(answer.error);
		if (answer.data.length === 0) {
			return { parts: [], error: `"${attachment.name}" rendered no pages, so it was not sent.` };
		}
		return {
			parts: answer.data.map((page) => ({
				mime: "image/png",
				data_base64: pngDataUrl(page.png),
				detail: null,
			})),
			error: "",
		};
	} catch (cause) {
		return {
			parts: [],
			error: `"${attachment.name}" could not be read from the ledger: ${messageOf(cause)}`,
		};
	}
}
