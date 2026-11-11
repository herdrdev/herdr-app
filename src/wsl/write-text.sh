set -eu
path=$1
present=$2
expected_len=$3
text_len=$4
work=$(mktemp -d)
temp=
trap 'rm -rf -- "$work"; if [ -n "$temp" ]; then rm -f -- "$temp"; fi' EXIT
head -c "$expected_len" > "$work/expected"
cat > "$work/new"
if [ "$(wc -c < "$work/expected")" -ne "$expected_len" ] || [ "$(wc -c < "$work/new")" -ne "$text_len" ]; then
    echo 'Incomplete file contents' >&2
    exit 1
fi
stale() {
    echo 'The file changed outside Herdr. Reload before saving; your draft is still here.' >&2
    exit 1
}
# A sentinel preserves trailing newlines in literal Linux filenames.
parent=$(dirname -- "$path"; printf '.')
parent=${parent%.}
parent=${parent%?}
if [ "$present" = 0 ]; then
    if [ -e "$path" ] || [ -L "$path" ]; then stale; fi
    mkdir -p -- "$parent"
    (set -C; cat "$work/new" > "$path")
else
    cmp -s -- "$path" "$work/expected" || stale
    target=$(readlink -f -- "$path" && printf '.')
    target=${target%.}
    target=${target%?}
    parent=$(dirname -- "$target"; printf '.')
    parent=${parent%.}
    parent=${parent%?}
    temp=$(mktemp "$parent/.herdr-app-write.XXXXXX")
    cat "$work/new" > "$temp"
    chmod "$(stat -c %a -- "$target")" "$temp"
    cmp -s -- "$target" "$work/expected" || stale
    mv -fT -- "$temp" "$target"
fi
