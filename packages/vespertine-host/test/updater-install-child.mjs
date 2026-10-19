// Run by updater.test.js in a child process whose environment names the AppImage to replace
// (APPIMAGE must be in the environment at process start; assigning to process.env later is not
// visible to native code under every runtime). Usage: <endpoint> <pubkey>
import { checkUpdate } from "../src/index.js";

const [endpoint, pubkey] = process.argv.slice(2);
const update = await checkUpdate("1.0.0", { endpoints: [endpoint], pubkey });
await update.downloadAndInstall();
// Installation neither quit nor relaunched this process.
console.log(`installed by ${process.pid}`);
