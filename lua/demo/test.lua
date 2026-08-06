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
