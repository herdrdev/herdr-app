// Pure routing policy, shared by the GPUI mod and tests. No Node APIs or globals.
export function isRemoteLoopbackUrl({ url, machine }) {
  if (!machine || typeof url !== "string") return false;
  const match = /^https?:\/\/([^/?#]+)/i.exec(url);
  if (!match) return false;
  const authority = match[1].slice(match[1].lastIndexOf("@") + 1);
  const host = authority.startsWith("[") ? authority.slice(0, authority.indexOf("]") + 1) : authority.split(":")[0];
  return /^(localhost\.?|\[::1\])$/i.test(host) || /^127(?:\.\d{1,3}){1,3}$/.test(host);
}
