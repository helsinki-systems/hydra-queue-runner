#!/usr/bin/env bash

set -euo pipefail

get_random_port() {
    local port_min=$1
    local port_max=$2
    shuf -i "${port_min}-${port_max}" -n 1
}

cleanup() {
    echo "Cleaning up processes..."
    if [[ -n "${QUEUE_RUNNER_PID:-}" ]]; then
        kill -9 "${QUEUE_RUNNER_PID}" 2>/dev/null || true
    fi
    if [[ -n "${BUILDER_PID:-}" ]]; then
        kill -9 "${BUILDER_PID}" 2>/dev/null || true
    fi
    wait 2>/dev/null || true
    echo "Cleanup complete"
}

if [[ $# -eq 0 ]]; then
    echo "Usage: $0 <build_id>"
    echo "Example: $0 12345"
    exit 1
fi

BUILD_ID="$1"

trap cleanup EXIT INT TERM

GRPC_PORT=$(get_random_port 5000 9999)
HTTP_PORT=$(get_random_port 10000 19999)

echo "Using GRPC port: ${GRPC_PORT}"
echo "Using HTTP port: ${HTTP_PORT}"

echo "Starting queue-runner..."
queue-runner \
    --rest-bind "[::]:${HTTP_PORT}" \
    --grpc-bind "[::]:${GRPC_PORT}" \
    --disable-queue-monitor-loop &
QUEUE_RUNNER_PID=$!

sleep 5
echo "Starting builder..."
builder --gateway-endpoint "http://[::]:${GRPC_PORT}" &
BUILDER_PID=$!

echo "Waiting for services to start..."
sleep 5

echo "Services started successfully!"
echo "queue-runner PID: ${QUEUE_RUNNER_PID}"
echo "builder PID: ${BUILDER_PID}"

# Function to submit build and monitor
submit_and_monitor_build() {
    local build_id="$1"

    echo "Submitting build ${build_id}..."

    curl -s --fail -X POST \
        --json "{\"buildId\": ${build_id}}" \
        "http://[::1]:${HTTP_PORT}/build_one"
    echo "Monitoring build ${build_id}..."

    while true; do
        local status_response
        status_response=$(curl -s "http://[::1]:${HTTP_PORT}/status/build/${build_id}/active")
        echo "Status response: ${status_response}"

        # Check if build is still active
        if [[ "${status_response}" == *"false"* ]] || [[ "${status_response}" == *"null"* ]] || [[ "${status_response}" == *"\"active\": false"* ]]; then
            echo "Build ${build_id} is no longer active"
            break
        elif [[ "${status_response}" == *"true"* ]] || [[ "${status_response}" == *"\"active\": true"* ]]; then
            echo "Build ${build_id} is still active, waiting..."
            sleep 10
        else
            echo "Unexpected status response: ${status_response}"
            sleep 5
        fi
    done

    echo "Build ${build_id} completed!"
}

submit_and_monitor_build "${BUILD_ID}"

echo "All done! Services will be cleaned up automatically."
