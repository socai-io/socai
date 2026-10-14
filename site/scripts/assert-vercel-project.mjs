const EXPECTED_PROJECT_ID = "prj_bI9zARRkb95vSg2I1cR3IA9DaVpJ";
const EXPECTED_PROJECT_NAME = "socai-site";

if (process.env.VERCEL === "1") {
  const actualProjectId = process.env.VERCEL_PROJECT_ID?.trim();
  if (!actualProjectId) {
    console.error(
      `[vercel-project-guard] Refusing to build ${EXPECTED_PROJECT_NAME}: ` +
        "VERCEL_PROJECT_ID is unavailable. Enable Vercel system environment variables.",
    );
    process.exit(1);
  }
  if (actualProjectId !== EXPECTED_PROJECT_ID) {
    console.error(
      `[vercel-project-guard] Refusing to build ${EXPECTED_PROJECT_NAME} in project ` +
        `${actualProjectId}; expected ${EXPECTED_PROJECT_ID}.`,
    );
    process.exit(1);
  }
  console.log(`[vercel-project-guard] Confirmed ${EXPECTED_PROJECT_NAME}.`);
}
