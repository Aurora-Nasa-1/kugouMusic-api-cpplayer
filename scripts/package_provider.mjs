#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const rootDir = path.resolve(__dirname, '..');

function parseArgs() {
    const args = process.argv.slice(2);
    const opts = {
        platform: 'windows',
        libPath: path.join(rootDir, 'target', 'release', 'kugou_music_rs.dll'),
        outZip: path.join(rootDir, 'target', 'kugou-provider.zip'),
        manifestPath: path.join(rootDir, 'packaging', 'manifest.json'),
    };
    for (let i = 0; i < args.length; i++) {
        if (args[i] === '--platform' && i + 1 < args.length) opts.platform = args[++i];
        else if (args[i] === '--lib' && i + 1 < args.length) opts.libPath = path.resolve(args[++i]);
        else if (args[i] === '--out' && i + 1 < args.length) opts.outZip = path.resolve(args[++i]);
        else if (args[i] === '--manifest' && i + 1 < args.length) opts.manifestPath = path.resolve(args[++i]);
    }
    return opts;
}

function findJarExecutable() {
    const javaHome = process.env.JAVA_HOME || 'E:\\java';
    const jarInHome = path.join(javaHome, 'bin', process.platform === 'win32' ? 'jar.exe' : 'jar');
    if (fs.existsSync(jarInHome)) return jarInHome;
    return 'jar';
}

function main() {
    const opts = parseArgs();
    console.log(`[package_provider] Starting package for platform: ${opts.platform}`);
    console.log(`[package_provider] Lib path: ${opts.libPath}`);

    if (!fs.existsSync(opts.libPath)) {
        console.error(`Error: Native library not found: ${opts.libPath}`);
        console.error('Please build first: cargo build --release --features jni');
        process.exit(1);
    }

    if (!fs.existsSync(opts.manifestPath)) {
        console.error(`Error: Manifest not found: ${opts.manifestPath}`);
        process.exit(1);
    }

    const rawManifest = fs.readFileSync(opts.manifestPath, 'utf8').replace(/^\uFEFF/, '');
    const manifest = JSON.parse(rawManifest);

    // Sync version from Cargo.toml if needed
    const cargoTomlPath = path.join(rootDir, 'Cargo.toml');
    if (fs.existsSync(cargoTomlPath)) {
        const cargoToml = fs.readFileSync(cargoTomlPath, 'utf8');
        const match = cargoToml.match(/^\s*version\s*=\s*"([^"]+)"/m);
        if (match && match[1]) {
            manifest.version = match[1];
        }
    }

    const libName = path.basename(opts.libPath);
    manifest.type = 'jni';
    manifest.contract = 'kmp-sdk-v1';
    manifest.entryPoint = libName;
    delete manifest.nativeFactoryClass;
    delete manifest.apiMap;

    const stagingDir = path.join(rootDir, 'target', 'package_staging');
    if (fs.existsSync(stagingDir)) {
        fs.rmSync(stagingDir, { recursive: true, force: true });
    }
    fs.mkdirSync(stagingDir, { recursive: true });

    // 1. Write clean manifest.json (UTF-8, no BOM)
    fs.writeFileSync(
        path.join(stagingDir, 'manifest.json'),
        JSON.stringify(manifest, null, 2) + '\n',
        'utf8'
    );

    // 2. Write entryPoint at root
    const rootLibDest = path.join(stagingDir, libName);
    fs.copyFileSync(opts.libPath, rootLibDest);

    // 3. Write lib/<abi>/<entryPoint>
    const abis = opts.platform === 'windows' ? ['x86_64', 'amd64'] : ['x86_64'];
    for (const abi of abis) {
        const abiDir = path.join(stagingDir, 'lib', abi);
        fs.mkdirSync(abiDir, { recursive: true });
        fs.copyFileSync(opts.libPath, path.join(abiDir, libName));
    }

    // 4. Create ZIP
    const outDir = path.dirname(opts.outZip);
    fs.mkdirSync(outDir, { recursive: true });
    if (fs.existsSync(opts.outZip)) {
        fs.unlinkSync(opts.outZip);
    }

    const jarExec = findJarExecutable();
    console.log(`[package_provider] Using jar tool: ${jarExec}`);
    execFileSync(jarExec, ['-cMf', opts.outZip, '-C', stagingDir, '.'], { stdio: 'inherit' });

    console.log(`[package_provider] Successfully created ZIP: ${opts.outZip} (${fs.statSync(opts.outZip).size} bytes)`);

    // 5. Inspect ZIP contents using jar -tf
    const list = execFileSync(jarExec, ['-tf', opts.outZip], { encoding: 'utf8' });
    console.log('[package_provider] ZIP Archive contents:');
    console.log(list.trim());
}

main();
