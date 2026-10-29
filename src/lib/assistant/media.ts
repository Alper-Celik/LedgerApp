// SPDX-FileCopyrightText: 2026 Alper Çelik <alper@alper-celik.dev>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * Files picked for one turn, read in the browser and handed to the backend as
 * base64 data parts.
 *
 * The picker accepts images, PDFs and CSVs, but the wire only has an image
 * part (`ai.rs`'s `MediaPart`), and the endpoint's own model list is the only
 * voice that says whether the selected model takes anything visual. So an
 * attachment is only sent when `aiModels` marked the model vision-capable;
 * otherwise the UI says so instead of shipping bytes the endpoint cannot read.
 *
 * Limits are deliberately the frontend's: the browser already has the bytes,
 * and a 10 MB file should be refused before it is encoded and passed over IPC.
 */
import type { ModelInfo } from "./api.js";

/** The most files one message may carry. */
export const MAX_ATTACHMENTS = 3;

/** The most one file may weigh. */
export const MAX_ATTACHMENT_BYTES = 10 * 1024 * 1024;

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
			attachments.push({ name: file.name, mime, size: file.size, dataUrl: await readAsDataUrl(file) });
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
