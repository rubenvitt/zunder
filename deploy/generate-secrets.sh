#!/bin/bash
set -e

# ============================================================================
# Secret Generation Script for Zunder
# ============================================================================
# Based on the docker-compose-deployment template, with three deliberate
# differences:
#   - The committed template .env.example is never modified. The filled-in
#     file is written to .env (ignored by git, mode 600), which compose.yaml
#     loads via env_file.
#   - Prompts go to stderr, so $(prompt_secret ...) captures only the value.
#   - Values are escaped for sed and replaced without `sed -i`, so tokens
#     containing / & | (e.g. Timing tokens like "123|abc") and macOS sed work.
# ============================================================================

echo "🔐 Secret Generation Script for Zunder"
echo "================================================"
echo ""

# Color codes for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# Work next to this script, regardless of the current directory
cd "$(dirname "$0")"

# Template (committed) and output (ignored by git)
TEMPLATE_FILE=".env.example"
ENV_FILE=".env"
TEMP_FILE="${ENV_FILE}.tmp"

# Function to generate random secret (alphanumeric, default 32 characters)
generate_secret() {
    local length=${1:-32}
    openssl rand -base64 $((length * 2)) | tr -d "=+/\n" | cut -c1-"$length"
}

# Function to prompt for secret (hidden input). Prompts go to stderr.
prompt_secret() {
    local var_name=$1
    local description=$2
    local secret_value

    echo -e "${YELLOW}Enter ${description} (${var_name}):${NC}" >&2
    read -rs secret_value
    echo "" >&2

    if [ -z "$secret_value" ]; then
        echo -e "${RED}❌ Error: Value cannot be empty${NC}" >&2
        exit 1
    fi

    echo "$secret_value"
}

# Function to prompt for a non-secret value (visible input)
prompt_value() {
    local var_name=$1
    local description=$2
    local value

    echo -e "${YELLOW}Enter ${description} (${var_name}):${NC}" >&2
    read -r value

    if [ -z "$value" ]; then
        echo -e "${RED}❌ Error: Value cannot be empty${NC}" >&2
        exit 1
    fi

    echo "$value"
}

# Function to prompt for an optional value (hidden input, may stay empty)
prompt_optional_secret() {
    local var_name=$1
    local description=$2
    local value

    echo -e "${YELLOW}Enter ${description} (${var_name}, leer lassen = aus):${NC}" >&2
    read -rs value
    echo "" >&2

    echo "$value"
}

# Function to prompt for an optional non-secret value (visible input)
prompt_optional_value() {
    local var_name=$1
    local description=$2
    local value

    echo -e "${YELLOW}Enter ${description} (${var_name}, leer lassen = aus):${NC}" >&2
    read -r value

    echo "$value"
}

# Escape a value for use as sed replacement with "|" as delimiter
escape_sed() {
    printf '%s' "$1" | sed -e 's/[\\|&]/\\&/g'
}

# Replace "NAME=PLACEHOLDER" with "NAME=value" in TEMP_FILE
set_var() {
    local name=$1
    local placeholder=$2
    local value=$3

    case "$value" in
        *$'\n'*)
            echo -e "${RED}❌ Error: ${name} must not contain line breaks${NC}" >&2
            exit 1
            ;;
    esac

    sed -e "s|^${name}=${placeholder}\$|${name}=$(escape_sed "$value")|" "$TEMP_FILE" > "${TEMP_FILE}.new"
    mv "${TEMP_FILE}.new" "$TEMP_FILE"
}

# Check if the template exists
if [ ! -f "$TEMPLATE_FILE" ]; then
    echo -e "${RED}❌ Error: $TEMPLATE_FILE not found${NC}"
    echo "Please run this script from a checkout of the Zunder repository"
    exit 1
fi

if ! command -v openssl > /dev/null 2>&1; then
    echo -e "${RED}❌ Error: openssl is required${NC}"
    exit 1
fi

if [ -f "$ENV_FILE" ]; then
    echo -e "${YELLOW}⚠️  $ENV_FILE exists and will be overwritten.${NC}"
    echo ""
fi

echo "📝 Generating secrets..."
echo ""

# Copy base template, readable only by the current user
umask 077
cp "$TEMPLATE_FILE" "$TEMP_FILE"
trap 'rm -f "$TEMP_FILE" "${TEMP_FILE}.new"' EXIT

# ============================================================================
# GENERATED variables
# ============================================================================
set_var ZUNDER_SESSION_SECRET WILL_BE_GENERATED "$(generate_secret 64)"

echo -e "${GREEN}✓ Generated random secrets${NC}"
echo ""

# ============================================================================
# MANUAL variables
# ============================================================================
echo "🔑 Please provide the following credentials:"
echo "   (Pocket-ID: OIDC-Client mit Callback https://zunder.rubeen.dev/auth/callback)"
echo ""

ZUNDER_OIDC_ISSUER=$(prompt_value "ZUNDER_OIDC_ISSUER" "Pocket-ID Issuer URL, e.g. https://id.example.com")
set_var ZUNDER_OIDC_ISSUER CHANGEME_MANUAL "$ZUNDER_OIDC_ISSUER"

ZUNDER_OIDC_CLIENT_ID=$(prompt_value "ZUNDER_OIDC_CLIENT_ID" "Pocket-ID Client ID")
set_var ZUNDER_OIDC_CLIENT_ID CHANGEME_MANUAL "$ZUNDER_OIDC_CLIENT_ID"

ZUNDER_OIDC_CLIENT_SECRET=$(prompt_secret "ZUNDER_OIDC_CLIENT_SECRET" "Pocket-ID Client Secret")
set_var ZUNDER_OIDC_CLIENT_SECRET CHANGEME_MANUAL "$ZUNDER_OIDC_CLIENT_SECRET"

ZUNDER_OIDC_ALLOWED_EMAIL=$(prompt_value "ZUNDER_OIDC_ALLOWED_EMAIL" "E-Mail of the Pocket-ID account allowed to log in")
set_var ZUNDER_OIDC_ALLOWED_EMAIL CHANGEME_MANUAL "$ZUNDER_OIDC_ALLOWED_EMAIL"

# ============================================================================
# OPTIONAL variables (empty = feature disabled)
# ============================================================================
echo ""
echo "⏱️  Timing-Brücke (optional):"
echo ""

ZUNDER_TIMING_TOKEN=$(prompt_optional_secret "ZUNDER_TIMING_TOKEN" "Timing API token")
set_var ZUNDER_TIMING_TOKEN CHANGEME_OPTIONAL "$ZUNDER_TIMING_TOKEN"

ZUNDER_TIMING_PROJECT=$(prompt_optional_value "ZUNDER_TIMING_PROJECT" "Timing project (self link /projects/<id> or project title)")
set_var ZUNDER_TIMING_PROJECT CHANGEME_OPTIONAL "$ZUNDER_TIMING_PROJECT"

# ============================================================================

# Every placeholder must be gone now
if grep -Eq '=(WILL_BE_GENERATED|CHANGEME_MANUAL|CHANGEME_OPTIONAL)$' "$TEMP_FILE"; then
    echo -e "${RED}❌ Error: unreplaced placeholders left:${NC}"
    grep -E '=(WILL_BE_GENERATED|CHANGEME_MANUAL|CHANGEME_OPTIONAL)$' "$TEMP_FILE" | cut -d= -f1
    exit 1
fi

# Move result into place (template stays untouched)
mv "$TEMP_FILE" "$ENV_FILE"

echo ""
echo "================================================"
echo -e "${GREEN}✅ All secrets configured!${NC}"
echo ""
echo "📄 Written: $(pwd)/$ENV_FILE (mode 600)"
echo ""
echo "Next steps (on the server, in ~/server/zunder):"
echo "   sudo chown 10001:10001 data   # once, the container runs as uid 10001"
echo "   docker compose pull && docker compose up -d"
echo ""
echo "⚠️  Security Reminders:"
echo "   - Never commit .env to git"
echo "   - If you generated it on another machine: copy it with scp, then delete the local copy"
