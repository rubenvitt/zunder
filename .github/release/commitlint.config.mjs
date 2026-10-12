// Conventional Commits for every commit of a pull request (ci.yml, job
// `commits`). Subjects are often German and start with a noun, so their
// case is free; long lines (URLs, trailers) are fine too.
export default {
  extends: ["@commitlint/config-conventional"],
  rules: {
    "subject-case": [0],
    "body-max-line-length": [0],
    "footer-max-line-length": [0],
  },
};
