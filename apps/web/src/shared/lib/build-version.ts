const SEMVER_LIKE_VERSION_PATTERN = /^\d+(?:\.\d+){1,3}(?:[-+][0-9A-Za-z.-]+)?$/;
const GIT_SHA_PATTERN = /^[0-9a-f]{7,40}$/i;

const normalizeBuildVersion = (value: string) => value.trim().replace(/^v(?=\d)/, "");

const formatBuildVersionLabel = (value: string) => {
  const normalized = normalizeBuildVersion(value);
  if (!normalized) {
    return "dev";
  }

  if (GIT_SHA_PATTERN.test(normalized)) {
    return normalized;
  }

  if (SEMVER_LIKE_VERSION_PATTERN.test(normalized)) {
    return `v${normalized}`;
  }

  return normalized;
};

export const BUILD_VERSION_LABEL = formatBuildVersionLabel(__APP_VERSION__);
