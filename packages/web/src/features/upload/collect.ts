/** A file plus where it should land relative to the drop target. */
export interface Picked {
  file: File;
  relPath: string;
  /** absolute path on disk (desktop app only) */
  absPath?: string;
  /** name of the top-level dropped directory this file came from */
  fromDir?: string;
}

export interface Collected {
  files: Picked[];
  /** top-level dropped directories (desktop: absolute paths) */
  dirs: { name: string; absPath?: string }[];
}

function readAll(reader: FileSystemDirectoryReader): Promise<FileSystemEntry[]> {
  return new Promise((resolve) => {
    const out: FileSystemEntry[] = [];
    const next = () =>
      reader.readEntries(
        (batch) => {
          if (!batch.length) resolve(out);
          else {
            out.push(...batch);
            next();
          }
        },
        () => resolve(out),
      );
    next();
  });
}

function fileOf(entry: FileSystemFileEntry): Promise<File | null> {
  return new Promise((resolve) => entry.file(resolve, () => resolve(null)));
}

async function walk(entry: FileSystemEntry, prefix: string, out: Picked[]) {
  if (entry.isFile) {
    const f = await fileOf(entry as FileSystemFileEntry);
    if (f && !/^(\.DS_Store|Thumbs\.db|desktop\.ini)$/i.test(f.name)) out.push({ file: f, relPath: prefix + f.name });
  } else if (entry.isDirectory) {
    const children = await readAll((entry as FileSystemDirectoryEntry).createReader());
    for (const c of children) await walk(c, `${prefix}${entry.name}/`, out);
  }
}

/** Collect files (recursively for folders) from a drop event, keeping relative paths. */
export async function collectFromDataTransfer(dt: DataTransfer, getPath?: (f: File) => string): Promise<Collected> {
  const items = Array.from(dt.items).filter((i) => i.kind === 'file');
  const entries = items.map((i) => ({ entry: i.webkitGetAsEntry?.() ?? null, file: i.getAsFile() }));
  const files: Picked[] = [];
  const dirs: Collected['dirs'] = [];
  for (const { entry, file } of entries) {
    if (entry?.isDirectory) {
      const abs = file && getPath ? getPath(file) : undefined;
      dirs.push({ name: entry.name, absPath: abs || undefined });
      const sub: Picked[] = [];
      await walk(entry, '', sub);
      files.push(...sub.map((s) => ({ ...s, absPath: undefined, fromDir: entry.name })));
    } else if (file) {
      const abs = getPath ? getPath(file) : undefined;
      files.push({ file, relPath: file.name, absPath: abs || undefined });
    }
  }
  return { files, dirs };
}

/** From an <input type=file> (webkitdirectory keeps relative paths). */
export function collectFromInput(list: FileList | null): Collected {
  const files: Picked[] = Array.from(list ?? []).map((f) => ({ file: f, relPath: (f as File & { webkitRelativePath?: string }).webkitRelativePath || f.name }));
  return { files, dirs: [] };
}
