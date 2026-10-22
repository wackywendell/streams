import { constants, closeSync, fstatSync, lstatSync, mkdirSync, openSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";

function outsideVersionControl(path: string): void {
 for (let current = path; ; current = dirname(current)) {
  for (const marker of [".git", ".jj"]) {
   try {lstatSync(join(current, marker));} catch (error) {if ((error as NodeJS.ErrnoException).code === "ENOENT") continue;throw error;}
   throw Error("Debug artifacts must stay outside version control");
  }
  if (dirname(current) === current) return;
 }
}
/** Only ancestors above the managed root may be trusted aliases (for example macOS /var). */
export function privateDirectory(path: string, managedRoot = path, create = true): string {
 const root = resolve(managedRoot), child = relative(root, resolve(path));
 if (child === ".." || child.startsWith(`..${sep}`) || isAbsolute(child)) throw Error("Private storage must stay within its managed root");
 let ancestor = dirname(root);
 const missing: string[] = [];
 while (true) {
  try {lstatSync(ancestor);break;} catch (error) {
   if ((error as NodeJS.ErrnoException).code !== "ENOENT" || !create) throw error;
   missing.unshift(basename(ancestor));
   ancestor = dirname(ancestor);
  }
 }
 let current = realpathSync(ancestor);
 outsideVersionControl(current);
 for (const part of missing) {
  current = join(current, part);mkdirSync(current, {mode: 0o700});
  outsideVersionControl(current);
 }
 for (const part of [basename(root), ...child.split(sep).filter(Boolean)]) {
  current = join(current, part);
  let stat;
  try {stat = lstatSync(current);} catch (error) {
   if (!create || (error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
   mkdirSync(current, {mode: 0o700});stat = lstatSync(current);
  }
  if (stat.isSymbolicLink() || !stat.isDirectory()) throw Error("Debug storage must use real directories, not symlinks");
  if ((stat.mode & 0o077) !== 0 || stat.uid !== process.getuid?.()) throw Error("Debug storage must be owned by this user with mode 0700");
  outsideVersionControl(current);
 }
 return current;
}
export function privateWrite(path: string, content: string, managedRoot = dirname(path)): void {
 const directory = privateDirectory(dirname(path), managedRoot);
 const fd = openSync(join(directory, basename(path)), constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
 try {writeFileSync(fd, content);} finally {closeSync(fd);}
}
export function privateRead(directory: string, file: string, managedRoot = directory): string {
 if (!file || isAbsolute(file) || file.includes("/") || file.includes("\\") || file === "." || file === "..") throw Error("Invalid private artifact path");
 const canonical = privateDirectory(directory, managedRoot, false), path = join(canonical, file);
 if (realpathSync(dirname(path)) !== canonical) throw Error("Invalid private artifact path");
 const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
 try {
  const stat = fstatSync(fd);
  if (!stat.isFile() || stat.uid !== process.getuid?.() || (stat.mode & 0o077) !== 0) throw Error("Private artifact must be a user-owned 0600 file");
  return readFileSync(fd, "utf8");
 } finally {closeSync(fd);}
}
