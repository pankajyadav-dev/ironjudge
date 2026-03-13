local current_key = KEYS[1]
local previous_key = KEYS[2]
local limit = tonumber(ARGV[1])
local previous_weight = tonumber(ARGV[2])
local expire_time = tonumber(ARGV[3])

local current_count = tonumber(redis.call('GET', current_key) or "0")
local previous_count = tonumber(redis.call('GET', previous_key) or "0")

local estimated_rate = (previous_count * previous_weight) + current_count

if estimated_rate >= limit then
    return 0
end

local new_count = redis.call('INCR', current_key)

if new_count == 1 then
    redis.call('EXPIRE', current_key, expire_time)
end

return 1



-- KEYS[1]: Current Bucket Key (e.g., "rate_limit:127.0.0.1:101")
-- KEYS[2]: Previous Bucket Key (e.g., "rate_limit:127.0.0.1:100")
-- ARGV[1]: Max limit (e.g., 100)
-- ARGV[2]: Weight of the previous window (e.g., 0.75)
-- ARGV[3]: Expiration time in seconds (Window Size * 2)
