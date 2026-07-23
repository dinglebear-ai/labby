<?php
/**
 * Authenticated aggregate-only status endpoint for LabbyDashboard.page.
 *
 * This file lives under emhttp's plugin tree, so normal webGUI session
 * authentication applies. It intentionally returns no upstream names, URLs,
 * commands, environment keys, credentials, logs, or tool arguments.
 */

header('Content-Type: application/json');
header('Cache-Control: no-store');

defined('LABBY_DASHBOARD_CFG') || define('LABBY_DASHBOARD_CFG', '/boot/config/plugins/labby/labby.cfg');
defined('LABBY_DASHBOARD_BIN') || define('LABBY_DASHBOARD_BIN', '/usr/local/emhttp/plugins/labby/bin/labby');
defined('LABBY_DASHBOARD_INCUS_ENV') || define('LABBY_DASHBOARD_INCUS_ENV', '/usr/local/emhttp/plugins/labby/scripts/labby-incus-env.sh');
defined('LABBY_DASHBOARD_RC') || define('LABBY_DASHBOARD_RC', '/etc/rc.d/rc.labby');

function labby_dashboard_cfg(): array
{
    $cfg = [];
    foreach (@file(LABBY_DASHBOARD_CFG, FILE_IGNORE_NEW_LINES | FILE_SKIP_EMPTY_LINES) ?: [] as $line) {
        $line = trim($line);
        if ($line === '' || $line[0] === '#') {
            continue;
        }
        $line = preg_replace('/\s+#.*$/', '', $line);
        if (preg_match('/^([A-Z_]+)="?([^"]*?)"?$/', $line, $match)) {
            $cfg[$match[1]] = $match[2];
        }
    }
    return [
        'RUNTIME_MODE' => ($cfg['RUNTIME_MODE'] ?? 'native') === 'incus' ? 'incus' : 'native',
        'INCUS_CONTAINER_NAME' => $cfg['INCUS_CONTAINER_NAME'] ?? 'labby-gateway',
        'SERVICE' => ($cfg['SERVICE'] ?? 'disabled') === 'enabled' ? 'enabled' : 'disabled',
        'LABBY_DIR' => $cfg['LABBY_DIR'] ?? '/mnt/user/appdata/labby',
        'HTTP_HOST' => in_array($cfg['HTTP_HOST'] ?? '', ['127.0.0.1', '0.0.0.0'], true)
            ? $cfg['HTTP_HOST']
            : '127.0.0.1',
        'HTTP_PORT' => preg_match('/^[0-9]{1,5}$/', $cfg['HTTP_PORT'] ?? '')
            ? $cfg['HTTP_PORT']
            : '8765',
    ];
}

function labby_dashboard_exec(array $parts, int &$exitCode): string
{
    $lines = [];
    exec(implode(' ', array_map('escapeshellarg', $parts)) . ' 2>/dev/null', $lines, $exitCode);
    return implode("\n", $lines);
}

function labby_dashboard_service_running(): bool
{
    $exitCode = 1;
    $output = labby_dashboard_exec(['timeout', '3', LABBY_DASHBOARD_RC, 'status'], $exitCode);
    return $exitCode === 0 && str_contains($output, 'RUNNING');
}

function labby_dashboard_state_dir_is_safe(string $dir): bool
{
    if (!preg_match('#^/mnt/(user|cache|disk[0-9]+)/[A-Za-z0-9_./-]+$#', $dir)) {
        return false;
    }
    foreach (explode('/', $dir) as $segment) {
        if ($segment === '.' || $segment === '..') {
            return false;
        }
    }
    return true;
}

function labby_dashboard_gateway_rows(array $cfg, int &$exitCode): array
{
    if ($cfg['RUNTIME_MODE'] === 'incus') {
        $container = $cfg['INCUS_CONTAINER_NAME'];
        if (!preg_match('/^[a-z]([a-z0-9-]{0,61}[a-z0-9])?$/', $container)) {
            $exitCode = 1;
            return [];
        }
        $script = '. "$1" && exec "$INCUS" exec "$2" --user labby -- env HOME=/home/labby XDG_CACHE_HOME=/home/labby/.cache XDG_CONFIG_HOME=/home/labby/.config XDG_DATA_HOME=/home/labby/.local/share PATH=/home/labby/.local/bin:/usr/local/bin:/usr/bin:/bin labby --json gateway mcp list';
        $output = labby_dashboard_exec([
            'timeout', '8', 'bash', '-c', $script, 'labby-dashboard',
            LABBY_DASHBOARD_INCUS_ENV, $container,
        ], $exitCode);
    } else {
        $dir = rtrim($cfg['LABBY_DIR'], '/');
        if (!labby_dashboard_state_dir_is_safe($dir)) {
            $exitCode = 1;
            return [];
        }
        $output = labby_dashboard_exec([
            'timeout', '8', 'env',
            'HOME=' . $dir,
            'XDG_CACHE_HOME=' . $dir . '/.cache',
            'XDG_CONFIG_HOME=' . $dir . '/.config',
            'XDG_DATA_HOME=' . $dir . '/.local/share',
            'LABBY_MCP_HTTP_HOST=' . $cfg['HTTP_HOST'],
            'LABBY_MCP_HTTP_PORT=' . $cfg['HTTP_PORT'],
            LABBY_DASHBOARD_BIN, '--json', 'gateway', 'mcp', 'list',
        ], $exitCode);
    }

    if ($exitCode !== 0) {
        return [];
    }
    $decoded = json_decode($output, true);
    if (!is_array($decoded) || !array_is_list($decoded)) {
        $exitCode = 1;
        return [];
    }
    foreach ($decoded as $row) {
        if (!is_array($row)) {
            $exitCode = 1;
            return [];
        }
    }
    return $decoded;
}

$cfg = labby_dashboard_cfg();
$running = labby_dashboard_service_running();
$rowsExitCode = 0;
$rows = $running ? labby_dashboard_gateway_rows($cfg, $rowsExitCode) : [];
$available = $running && $rowsExitCode === 0;
$enabled = $available ? count(array_filter($rows, fn($row) => !empty($row['enabled']))) : 0;
$connected = $available ? count(array_filter($rows, fn($row) => !empty($row['connected']))) : 0;
$errors = $available ? count(array_filter(
    $rows,
    fn($row) => !empty($row['enabled']) && (empty($row['connected']) || !empty($row['last_error']))
)) : 0;
$tools = $available ? array_sum(array_map(fn($row) => (int) ($row['exposed_tool_count'] ?? 0), $rows)) : 0;

echo json_encode([
    'service' => [
        'enabled' => $cfg['SERVICE'] === 'enabled',
        'running' => $running,
    ],
    'runtime' => $cfg['RUNTIME_MODE'],
    'gateway' => [
        'available' => $available,
        'total' => $available ? count($rows) : 0,
        'enabled' => $enabled,
        'connected' => $connected,
        'tools' => $tools,
        'errors' => $errors,
    ],
    'checkedAt' => gmdate('c'),
], JSON_UNESCAPED_SLASHES);
