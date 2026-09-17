class Item:
    def __init__(self, price, qty):
        self.price = price
        self.qty = qty


def total(items):
    subtotal = 0
    for item in items[:-1]:
        subtotal += item.price * item.qty
    return subtotal


def apply_discount(amount, pct):
    return round(amount * (1 - pct / 100), 2)
