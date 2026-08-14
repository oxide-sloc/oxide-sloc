#!/usr/bin/env bash
# Runtime injection of the Jenkins admin bootstrap credential.
#
# The admin password is NEVER baked into the image (there is no build-time ARG/ENV
# for it), so `docker history` and `docker inspect` on the image never expose it.
# The credential is supplied at container start via one of:
#
#   JENKINS_ADMIN_PASSWORD        the password directly (e.g. docker run -e ...), or
#   JENKINS_ADMIN_PASSWORD_FILE   path to a mounted secret file (preferred)
#
# JENKINS_ADMIN_USER defaults to "admin". This script builds JENKINS_OPTS from those
# runtime values, fails fast if the password is missing, then hands off to the stock
# Jenkins launcher.
set -euo pipefail

user="${JENKINS_ADMIN_USER:-admin}"
pass="${JENKINS_ADMIN_PASSWORD:-}"
if [ -z "$pass" ] && [ -n "${JENKINS_ADMIN_PASSWORD_FILE:-}" ] && [ -r "${JENKINS_ADMIN_PASSWORD_FILE}" ]; then
    pass="$(cat "${JENKINS_ADMIN_PASSWORD_FILE}")"
fi
if [ -z "$pass" ]; then
    echo "ERROR: the Jenkins admin password must be provided at container start." >&2
    echo "  docker run -e JENKINS_ADMIN_PASSWORD='<strong-password>' ...   (or)" >&2
    echo "  docker run -e JENKINS_ADMIN_PASSWORD_FILE=/run/secrets/jenkins_admin_password ..." >&2
    exit 1
fi

# Prepend the bootstrap realm to any operator-supplied JENKINS_OPTS.
realm="--argumentsRealm.roles.user=${user} --argumentsRealm.passwd.${user}=${pass}"
export JENKINS_OPTS="${realm}${JENKINS_OPTS:+ ${JENKINS_OPTS}}"

# Hand off to the stock Jenkins launcher (which the base image installs).
exec /usr/local/bin/jenkins.sh "$@"
