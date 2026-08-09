-- Rustrapper Lua demo: downloaded via PXE and executed on-device
function fib(n)
    if n < 2 then return n end
    return fib(n - 1) + fib(n - 2)
end

print("Hello from Lua!")
print("fib(10) = " .. fib(10))

t = {name = "rustrapper", arch = "arm64", values = {1, 2, 3}}
print(t.name, t.arch, t.values[2])

sum = 0
for i = 1, 5 do
    sum = sum + i
end
print("sum = " .. sum)

-- fetch(): download sample files from the TFTP server and print byte counts.
-- Returns the size on success or nil if the download failed.
local f1 = fetch("test.txt")
if f1 == nil then
    print("fetch test.txt: FAILED")
else
    print("fetch test.txt: " .. f1 .. " bytes")
end

local f2 = fetch("rust_payload.bin")
if f2 == nil then
    print("fetch rust_payload.bin: FAILED")
else
    print("fetch rust_payload.bin: " .. f2 .. " bytes")
end
