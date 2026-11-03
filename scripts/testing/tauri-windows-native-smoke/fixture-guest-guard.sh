expected=$1
shift
if [ -n "$expected" ] && [ "${WSL_DISTRO_NAME-}" = "$expected" ]; then
  exec "$@"
fi
printf '%s\n' 'fixture target identity mismatch' >&2
exit 90
